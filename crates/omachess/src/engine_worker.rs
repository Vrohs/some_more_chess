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
use omachess_core::review::{analyse_game, GameAnalysis};
use shakmaty::Color;

pub enum Request {
    /// Choose a move for the engine to play, at a capped strength.
    Move {
        fen: String,
        moves: Vec<String>,
        elo: u32,
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
    Review(GameAnalysis),
    Failed(String),
}

/// How long the engine gets to choose a move. Enough to play sensibly at a
/// capped rating, short enough that the game does not drag.
const MOVE_TIME: Duration = Duration::from_millis(400);

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
            let mut engine = match Engine::spawn(&path) {
                Ok(engine) => engine,
                Err(e) => {
                    let _ = reply_tx.send(Reply::Failed(format!("{e:#}")));
                    return;
                }
            };
            // A capped opponent and a full-strength analyst are the same binary
            // reconfigured, so the current cap is tracked to avoid needless
            // option changes between moves.
            let mut current_elo: Option<u32> = None;

            while let Ok(request) = request_rx.recv() {
                let reply = match request {
                    Request::Move { fen, moves, elo } => {
                        if current_elo != Some(elo) {
                            if let Err(e) = engine.limit_strength(Some(elo)) {
                                let _ = reply_tx.send(Reply::Failed(format!("{e:#}")));
                                continue;
                            }
                            current_elo = Some(elo);
                        }
                        match engine.analyse(&fen, &moves, Limit::Movetime(MOVE_TIME)) {
                            Ok(analysis) => match analysis.best_move {
                                Some(mv) => Reply::Move(mv),
                                None => Reply::Failed("engine returned no move".into()),
                            },
                            Err(e) => Reply::Failed(format!("{e:#}")),
                        }
                    }
                    Request::Review { fen, moves, player } => {
                        if current_elo.is_some() {
                            if let Err(e) = engine.limit_strength(None) {
                                let _ = reply_tx.send(Reply::Failed(format!("{e:#}")));
                                continue;
                            }
                            current_elo = None;
                        }
                        match analyse_game(&mut engine, &fen, &moves, player) {
                            Ok(analysis) => Reply::Review(analysis),
                            Err(e) => Reply::Failed(format!("{e:#}")),
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

    /// A worker whose engine never starts must report the failure once and then
    /// fall silent. Reporting it forever turns any `while let Some(..) = poll()`
    /// into an infinite loop, and on the UI thread that is a frozen window.
    #[test]
    fn a_dead_worker_reports_once_then_stops() {
        let worker = EngineWorker::spawn(PathBuf::from("/nonexistent/definitely-not-an-engine"));

        // Give the thread a moment to fail and exit, collecting everything it
        // says. Exactly one failure should come out, however it exits.
        let mut replies = Vec::new();
        for _ in 0..200 {
            while let Some(reply) = worker.poll() {
                replies.push(reply);
                if replies.len() > 8 {
                    panic!("worker produced a flood of replies");
                }
            }
            if worker.is_finished() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(replies.len(), 1, "expected exactly one failure report");
        assert!(matches!(replies[0], Reply::Failed(_)));

        // Every subsequent poll must be None, however many times it is called.
        for i in 0..1000 {
            assert!(
                worker.poll().is_none(),
                "worker kept reporting after it died (poll {i})"
            );
        }
        assert!(worker.is_finished());
    }

    #[test]
    fn sending_to_a_dead_worker_is_refused_rather_than_blocking() {
        let worker = EngineWorker::spawn(PathBuf::from("/nonexistent/definitely-not-an-engine"));
        std::thread::sleep(std::time::Duration::from_millis(200));
        // The request channel is closed with the thread; this must return
        // rather than wait for a reader that will never come.
        let _ = worker.send(Request::Move {
            fen: omachess_core::game::START_FEN.to_owned(),
            moves: Vec::new(),
            elo: 1320,
        });
    }
}
