//! Running the engine off the UI thread.
//!
//! Engine calls take seconds. Doing them on the GTK main loop would freeze the
//! window mid-game, so the engine lives on its own thread and the UI polls for
//! replies.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use omachess_core::engine::{Engine, Limit};
use omachess_core::review::{analyse_game, confirm_drillable, AtDepth, GameAnalysis, CONFIRM_DEPTH};
use shakmaty::Color;

pub enum Request {
    /// Choose a move for the engine to play, at a capped strength.
    Move {
        fen: String,
        moves: Vec<String>,
        elo: u32,
    },
    /// Evaluate one position as deeply as asked, for studying a game.
    Evaluate {
        fen: String,
        moves: Vec<String>,
        depth: u32,
        /// Echoed back, so a reply arriving after the board has moved on can
        /// be discarded rather than shown against the wrong position.
        token: u64,
    },
    /// Review a finished game at full strength.
    Review {
        fen: String,
        moves: Vec<String>,
        player: Color,
    },
}

pub enum Reply {
    Move(String),
    /// An evaluation, tagged with the token of the request that asked for it.
    Evaluation {
        analysis: omachess_core::engine::Analysis,
        token: u64,
    },
    Review(GameAnalysis),
    Failed(String),
}

/// How long the engine gets to choose a move. Enough to play sensibly at a
/// capped rating, short enough that the game does not drag.
const MOVE_TIME: Duration = Duration::from_millis(400);

/// Longest a study evaluation may take. Deep analysis is worth waiting a moment
/// for; it is not worth a position that never comes back.
const STUDY_TIME_CAP_MS: u64 = 2500;

pub struct EngineWorker {
    requests: Sender<Request>,
    replies: Receiver<Reply>,
    name: String,
    /// Set once the worker thread is gone. Without this the disconnected
    /// channel yields an error on every poll, and a caller draining replies in
    /// a loop never terminates — freezing the UI thread it runs on.
    finished: Cell<bool>,
    /// Whether any reply has already been handed out. The worker reports its
    /// own failure before exiting, so synthesising a second one on disconnect
    /// would duplicate it; synthesising none would hide a thread that died
    /// without saying anything.
    delivered: Cell<bool>,
}

impl EngineWorker {
    /// Start a worker around the engine at `path`.
    pub fn spawn(path: PathBuf) -> Self {
        let (request_tx, request_rx) = channel::<Request>();
        let (reply_tx, reply_rx) = channel::<Reply>();

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "engine".into());

        thread::spawn(move || {
            // The engine is started lazily and replaced if it dies, so one bad
            // spawn — a busy sandbox, a killed process — does not end play for
            // the rest of the session.
            let mut engine: Option<Engine> = None;
            let mut current_elo: Option<u32> = None;

            while let Ok(request) = request_rx.recv() {
                if engine.is_none() {
                    match Engine::spawn(&path) {
                        Ok(started) => {
                            engine = Some(started);
                            current_elo = None;
                        }
                        Err(e) => {
                            if reply_tx.send(Reply::Failed(format!("{e:#}"))).is_err() {
                                return;
                            }
                            continue;
                        }
                    }
                }
                let Some(active) = engine.as_mut() else {
                    continue;
                };

                let reply = match request {
                    Request::Move { fen, moves, elo } => {
                        if current_elo != Some(elo) {
                            match active.limit_strength(Some(elo)) {
                                Ok(()) => current_elo = Some(elo),
                                Err(e) => {
                                    engine = None;
                                    let _ = reply_tx.send(Reply::Failed(format!("{e:#}")));
                                    continue;
                                }
                            }
                        }
                        match active.analyse(&fen, &moves, Limit::Movetime(MOVE_TIME)) {
                            Ok(analysis) => match analysis.best_move {
                                Some(mv) => Reply::Move(mv),
                                None => Reply::Failed("the engine returned no move".into()),
                            },
                            Err(e) => {
                                // A protocol error means the process is no
                                // longer trustworthy; start a fresh one next time.
                                engine = None;
                                Reply::Failed(format!("{e:#}"))
                            }
                        }
                    }
                    Request::Evaluate {
                        fen,
                        moves,
                        depth,
                        token,
                    } => {
                        if current_elo.is_some() {
                            match active.limit_strength(None) {
                                Ok(()) => current_elo = None,
                                Err(e) => {
                                    engine = None;
                                    let _ = reply_tx.send(Reply::Failed(format!("{e:#}")));
                                    continue;
                                }
                            }
                        }
                        match active.analyse(
                            &fen,
                            &moves,
                            Limit::DepthOrTime {
                                depth,
                                millis: STUDY_TIME_CAP_MS,
                            },
                        ) {
                            Ok(analysis) => Reply::Evaluation { analysis, token },
                            Err(e) => {
                                engine = None;
                                Reply::Failed(format!("{e:#}"))
                            }
                        }
                    }
                    Request::Review { fen, moves, player } => {
                        if current_elo.is_some() {
                            match active.limit_strength(None) {
                                Ok(()) => current_elo = None,
                                Err(e) => {
                                    engine = None;
                                    let _ = reply_tx.send(Reply::Failed(format!("{e:#}")));
                                    continue;
                                }
                            }
                        }
                        match analyse_game(active, &fen, &moves, player) {
                            Ok(mut analysis) => {
                                // Anything that will be offered as a puzzle is
                                // re-checked deeper before it can be.
                                let mut deeper = AtDepth {
                                    engine: active,
                                    depth: CONFIRM_DEPTH,
                                };
                                let _ = confirm_drillable(&mut deeper, &mut analysis);
                                Reply::Review(analysis)
                            }
                            Err(e) => {
                                engine = None;
                                Reply::Failed(format!("{e:#}"))
                            }
                        }
                    }
                };
                if reply_tx.send(reply).is_err() {
                    break;
                }
            }
        });

        Self {
            requests: request_tx,
            replies: reply_rx,
            name,
            finished: Cell::new(false),
            delivered: Cell::new(false),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Queue work. Returns false once the worker has gone away.
    pub fn send(&self, request: Request) -> bool {
        self.requests.send(request).is_ok()
    }

    /// Take one reply if the worker has produced any.
    pub fn poll(&self) -> Option<Reply> {
        if self.finished.get() {
            return None;
        }
        match self.replies.try_recv() {
            Ok(reply) => {
                self.delivered.set(true);
                Some(reply)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.finished.set(true);
                if self.delivered.get() {
                    None
                } else {
                    self.delivered.set(true);
                    Some(Reply::Failed("the engine stopped".into()))
                }
            }
        }
    }

    /// Whether the worker has gone away for good.
    pub fn is_finished(&self) -> bool {
        self.finished.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(worker: &EngineWorker, budget: usize) -> Vec<Reply> {
        let mut out = Vec::new();
        for _ in 0..budget {
            while let Some(reply) = worker.poll() {
                out.push(reply);
                assert!(out.len() < 16, "worker produced a flood of replies");
            }
            if !out.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        out
    }

    fn request() -> Request {
        Request::Move {
            fen: omachess_core::game::START_FEN.to_owned(),
            moves: Vec::new(),
            elo: 1320,
        }
    }

    /// A failing engine must produce one report per request, never a stream.
    /// A stream turns any `while let Some(..) = poll()` into an infinite loop,
    /// and on the UI thread that is a frozen window.
    #[test]
    fn a_failing_engine_reports_once_per_request_not_continuously() {
        let worker = EngineWorker::spawn(PathBuf::from("/nonexistent/definitely-not-an-engine"));
        assert!(worker.send(request()));

        let replies = drain(&worker, 200);
        assert_eq!(replies.len(), 1, "expected exactly one failure");
        assert!(matches!(replies[0], Reply::Failed(_)));

        for i in 0..1000 {
            assert!(worker.poll().is_none(), "kept talking after the report (poll {i})");
        }
    }

    /// And it must stay usable: a bad spawn should not end play for the rest of
    /// the session, so the next request tries again.
    #[test]
    fn a_failed_spawn_is_retried_on_the_next_request() {
        let worker = EngineWorker::spawn(PathBuf::from("/nonexistent/definitely-not-an-engine"));
        assert!(worker.send(request()));
        assert_eq!(drain(&worker, 200).len(), 1);

        assert!(worker.send(request()), "the worker should still accept work");
        let second = drain(&worker, 200);
        assert_eq!(second.len(), 1, "the retry should report too");
        assert!(matches!(second[0], Reply::Failed(_)));
        assert!(!worker.is_finished(), "the worker must still be alive");
    }

    #[test]
    fn sending_never_blocks_the_caller() {
        let worker = EngineWorker::spawn(PathBuf::from("/nonexistent/definitely-not-an-engine"));
        // Queueing work must return immediately whatever the engine is doing;
        // this runs on the UI thread.
        for _ in 0..50 {
            let _ = worker.send(request());
        }
    }
}
