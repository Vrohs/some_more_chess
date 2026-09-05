//! Studying a game move by move, with the engine explaining as you go.
//!
//! Any PGN works: your own games, or the games of the masters. The engine
//! evaluates the position in front of you at a much deeper search than the
//! bulk review uses, because here there is exactly one position to think about
//! and time to spend on it.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk4::prelude::*;
use gtk4::{
    gdk, glib, Align, AspectFrame, Box as GtkBox, Button, DropDown, EventControllerKey, Label,
    Orientation, PolicyType, ScrolledWindow,
};
use omachess_core::engine::{Analysis, Score};
use omachess_core::game::START_FEN;
use omachess_core::openings;
use omachess_core::pgn::ImportedGame;
use omachess_core::study::Walkthrough;
use shakmaty::san::San;
use shakmaty::uci::UciMove;
use shakmaty::{Chess, Position};

use crate::board::BoardView;
use crate::engine_worker::{EngineWorker, Reply, Request};
use crate::pieces::PieceSet;

/// How deep the engine looks at a position being studied. Far deeper than the
/// bulk review, because there is one position and no hurry.
const STUDY_DEPTH: u32 = 22;
/// How often replies are collected.
const POLL_MS: u32 = 80;

pub struct StudyView {
    root: GtkBox,
    board: Rc<BoardView>,
    store: Rc<RefCell<omachess_core::store::Store>>,
    /// The sitting these positions belong to.
    sitting: Cell<Option<i64>>,
    /// When the position now on the board was reached, so time spent looking
    /// at it can be recorded. Dwell is the whole signal here: a study session
    /// has no right answers to score, only where the attention went.
    arrived: Cell<Option<std::time::Instant>>,
    worker: Option<EngineWorker>,
    games: RefCell<Vec<ImportedGame>>,
    walk: RefCell<Option<Walkthrough>>,
    picker: DropDown,
    heading: Label,
    opening: Label,
    evaluation: Label,
    best: Label,
    variation: Label,
    moves: Label,
    prev: Button,
    next: Button,
    /// Identifies the request for the position on the board, so a slow reply
    /// for a position already left behind is discarded instead of shown.
    token: Cell<u64>,
    /// Whether the engine is busy. Only one request is ever outstanding:
    /// queueing one per keypress means stepping through a game faster than the
    /// engine can think builds a backlog of answers nobody will ever see, which
    /// looks exactly like the application having stopped.
    busy: Cell<bool>,
    /// Set when the board moved on while the engine was busy, so the position
    /// actually on screen is asked about as soon as it is free.
    pending: Cell<bool>,
}

impl StudyView {
    pub fn new(
        store: Rc<RefCell<omachess_core::store::Store>>,
        pieces: Option<Rc<PieceSet>>,
        engine: Option<std::path::PathBuf>,
    ) -> Rc<Self> {
        let board = BoardView::new(pieces);
        let worker = engine.map(EngineWorker::spawn);

        let open = Button::with_label("Open PGN…");
        open.add_css_class("suggested-action");
        let picker = DropDown::from_strings(&[]);
        picker.set_visible(false);

        let first = Button::with_label("⏮");
        let prev = Button::with_label("‹ Prev");
        let next = Button::with_label("Next ›");
        let last = Button::with_label("⏭");
        for button in [&first, &prev, &next, &last] {
            button.set_sensitive(false);
        }

        let heading = Label::builder()
            .label("Open a PGN to study it — your games, or anyone's.")
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(38)
            .build();
        heading.add_css_class("omachess-status");

        let opening = Label::builder()
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(38)
            .build();
        opening.add_css_class("dim-label");

        let evaluation = Label::builder().halign(Align::Start).build();
        evaluation.add_css_class("title-2");

        let best = Label::builder()
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(38)
            .build();
        let variation = Label::builder()
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(38)
            .build();
        variation.add_css_class("dim-label");

        let moves = Label::builder()
            .halign(Align::Start)
            .valign(Align::Start)
            .wrap(true)
            .selectable(true)
            .build();
        moves.add_css_class("monospace");

        let controls = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(6)
            .build();
        controls.append(&open);
        controls.append(&first);
        controls.append(&prev);
        controls.append(&next);
        controls.append(&last);

        let framed = AspectFrame::builder()
            .ratio(1.0)
            .obey_child(false)
            .hexpand(true)
            .vexpand(true)
            .child(board.widget())
            .build();

        let move_scroll = ScrolledWindow::builder()
            .child(&moves)
            .hscrollbar_policy(PolicyType::Never)
            .vexpand(true)
            .build();

        let panel = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(8)
            .width_request(260)
            .build();
        panel.append(&controls);
        panel.append(&picker);
        panel.append(&heading);
        panel.append(&opening);
        panel.append(&evaluation);
        panel.append(&best);
        panel.append(&variation);
        panel.append(&move_scroll);

        let panel_scroll = ScrolledWindow::builder()
            .child(&panel)
            .hscrollbar_policy(PolicyType::Never)
            .propagate_natural_width(true)
            .build();

        let content = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(16)
            .build();
        content.append(&framed);
        content.append(&panel_scroll);

        let root = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        root.append(&content);

        let view = Rc::new(Self {
            root,
            board,
            store,
            sitting: Cell::new(None),
            arrived: Cell::new(None),
            worker,
            games: RefCell::new(Vec::new()),
            walk: RefCell::new(None),
            picker,
            heading,
            opening,
            evaluation,
            best,
            variation,
            moves,
            prev: prev.clone(),
            next: next.clone(),
            token: Cell::new(0),
            busy: Cell::new(false),
            pending: Cell::new(false),
        });

        let weak: Weak<Self> = Rc::downgrade(&view);
        open.connect_clicked(move |button| {
            if let Some(view) = weak.upgrade() {
                view.choose_file(button);
            }
        });

        for (button, step) in [
            (&first, Step::Start),
            (&prev, Step::Back),
            (&next, Step::Forward),
            (&last, Step::End),
        ] {
            let weak: Weak<Self> = Rc::downgrade(&view);
            button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.step(step);
                }
            });
        }

        let weak: Weak<Self> = Rc::downgrade(&view);
        view.picker.connect_selected_notify(move |picker| {
            if let Some(view) = weak.upgrade() {
                view.load_game(picker.selected() as usize);
            }
        });

        // Arrow keys walk the game, which is how anyone reads through one.
        let keys = EventControllerKey::new();
        let weak: Weak<Self> = Rc::downgrade(&view);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let step = match key {
                gdk::Key::Left => Step::Back,
                gdk::Key::Right => Step::Forward,
                gdk::Key::Home => Step::Start,
                gdk::Key::End => Step::End,
                _ => return glib::Propagation::Proceed,
            };
            view.step(step);
            glib::Propagation::Stop
        });
        view.root.add_controller(keys);

        let weak: Weak<Self> = Rc::downgrade(&view);
        glib::timeout_add_local(
            std::time::Duration::from_millis(u64::from(POLL_MS)),
            move || match weak.upgrade() {
                Some(view) => {
                    view.drain_engine();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            },
        );

        if view.worker.is_none() {
            view.heading
                .set_label("No engine found. Install stockfish to study with analysis.");
        }
        view
    }

    pub fn widget(&self) -> &GtkBox {
        &self.root
    }

    fn choose_file(self: &Rc<Self>, anchor: &Button) {
        let dialog = gtk4::FileDialog::builder().title("Open a PGN").build();
        let window = anchor.root().and_downcast::<gtk4::Window>();
        let weak: Weak<Self> = Rc::downgrade(self);
        dialog.open(
            window.as_ref(),
            gtk4::gio::Cancellable::NONE,
            move |result| {
                let Some(view) = weak.upgrade() else {
                    return;
                };
                let Ok(file) = result else {
                    return; // Cancelled.
                };
                let Some(path) = file.path() else {
                    return;
                };
                view.load_file(&path);
            },
        );
    }

    /// Open a PGN by path, for a file named on the command line.
    pub fn open_path(self: &Rc<Self>, path: &std::path::Path) {
        self.load_file(path);
    }

    fn load_file(self: &Rc<Self>, path: &std::path::Path) {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.heading
                    .set_label(&format!("Could not read that file: {e}"));
                return;
            }
        };
        let games = match omachess_core::pgn::read_all(&bytes) {
            Ok(games) => games,
            Err(e) => {
                self.heading
                    .set_label(&format!("Could not read that PGN: {e}"));
                return;
            }
        };
        if games.is_empty() {
            self.heading.set_label("No playable games in that file.");
            return;
        }

        let labels: Vec<String> = games
            .iter()
            .enumerate()
            .map(|(i, g)| {
                format!(
                    "{}. {} vs {} · {}",
                    i + 1,
                    if g.white.is_empty() { "?" } else { &g.white },
                    if g.black.is_empty() { "?" } else { &g.black },
                    g.result
                )
            })
            .collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        self.picker.set_model(Some(&gtk4::StringList::new(&refs)));
        self.picker.set_visible(games.len() > 1);

        *self.games.borrow_mut() = games;
        self.load_game(0);
    }

    fn load_game(self: &Rc<Self>, index: usize) {
        let games = self.games.borrow();
        let Some(game) = games.get(index) else {
            return;
        };
        match Walkthrough::new(START_FEN, &game.moves) {
            Ok(walk) => {
                self.heading.set_label(&format!(
                    "{} vs {} · {}",
                    game.white, game.black, game.result
                ));
                drop(games);
                if self.sitting.get().is_none() {
                    match self
                        .store
                        .borrow()
                        .begin_session("study", chrono::Utc::now())
                    {
                        Ok(id) => self.sitting.set(Some(id)),
                        Err(e) => {
                            omachess_core::diagnostics::record_error("study::begin_session", e)
                        }
                    }
                }
                self.arrived.set(Some(std::time::Instant::now()));
                *self.walk.borrow_mut() = Some(walk);
                self.refresh();
            }
            Err(e) => self.heading.set_label(&format!("Could not replay it: {e}")),
        }
    }

    fn step(self: &Rc<Self>, step: Step) {
        self.log_dwell(step);
        {
            let mut walk = self.walk.borrow_mut();
            let Some(walk) = walk.as_mut() else {
                return;
            };
            match step {
                Step::Forward => walk.forward(),
                Step::Back => walk.back(),
                Step::Start => {
                    walk.go_to_start();
                    true
                }
                Step::End => {
                    walk.go_to_end();
                    true
                }
            };
        }
        self.refresh();
    }

    /// Record how long the position being left was looked at.
    ///
    /// There is nothing to score in study — no right answer, no clock — so
    /// attention is the measurement: which positions were sat with, and which
    /// were stepped straight past.
    fn log_dwell(&self, step: Step) {
        let Some(arrived) = self.arrived.replace(Some(std::time::Instant::now())) else {
            return;
        };
        let Some(sitting) = self.sitting.get() else {
            return;
        };
        let (subject, ply, played) = {
            let walk = self.walk.borrow();
            match walk.as_ref() {
                Some(walk) => (
                    walk.played_san().join(" "),
                    walk.index() as u32,
                    walk.moves_so_far().last().cloned().unwrap_or_default(),
                ),
                None => return,
            }
        };
        // The subject is the game; a whole move list would make the log
        // unreadable, so it is identified by its opening moves.
        let subject: String = subject
            .split_whitespace()
            .take(6)
            .collect::<Vec<_>>()
            .join(" ");
        let detail = format!(
            "{{\"direction\":\"{}\"}}",
            match step {
                Step::Forward => "forward",
                Step::Back => "back",
                Step::Start => "start",
                Step::End => "end",
            }
        );
        if let Err(e) = self.store.borrow().log_move(
            Some(sitting),
            "study",
            &subject,
            chrono::Utc::now(),
            ply,
            &played,
            arrived.elapsed(),
            &detail,
        ) {
            omachess_core::diagnostics::record_error("study::log_dwell", e);
        }
    }

    /// Redraw everything for the position now being looked at, and ask the
    /// engine what it thinks of it.
    fn refresh(self: &Rc<Self>) {
        let Some((position, last, played, moves_so_far, next_san, index, total, at_start, at_end)) =
            ({
                let walk = self.walk.borrow();
                walk.as_ref().map(|w| {
                    (
                        w.position().clone(),
                        w.last_move(),
                        w.played_san().to_vec(),
                        w.moves_so_far().to_vec(),
                        w.next_san().map(str::to_owned),
                        w.index(),
                        w.len(),
                        w.at_start(),
                        w.at_end(),
                    )
                })
            })
        else {
            return;
        };

        self.board.set_orientation(shakmaty::Color::White);
        self.board.set_position(&position);
        self.board.set_last_move(last);
        self.board.set_check(
            position
                .is_check()
                .then(|| position.board().king_of(position.turn()))
                .flatten(),
        );
        self.prev.set_sensitive(!at_start);
        self.next.set_sensitive(!at_end);

        self.moves.set_label(&format_moves(&played, index));
        self.opening.set_label(
            &openings::identify(&moves_so_far)
                .map(|o| format!("{} ({})", o.name, o.eco))
                .unwrap_or_default(),
        );

        let position_of = format!("Move {} of {total}", index);
        self.evaluation.set_label(&position_of);
        self.best.set_label(&match next_san {
            Some(san) => format!("Played here: {san}"),
            None => "End of the game.".to_owned(),
        });
        self.variation.set_label("Thinking…");

        if self.worker.is_none() {
            self.variation.set_label("");
            return;
        }
        self.request_evaluation();
    }

    /// Ask about the position on the board, unless the engine is already busy —
    /// in which case note that it should be asked again the moment it is free.
    fn request_evaluation(&self) {
        let Some(worker) = &self.worker else {
            return;
        };
        if self.busy.get() {
            self.pending.set(true);
            return;
        }
        let Some(moves) = self
            .walk
            .borrow()
            .as_ref()
            .map(|w| w.moves_so_far().to_vec())
        else {
            return;
        };
        // Tag the request so a reply for a position already left cannot be
        // shown against this one.
        let token = self.token.get().wrapping_add(1);
        self.token.set(token);
        self.busy.set(true);
        self.pending.set(false);
        worker.send(Request::Evaluate {
            fen: START_FEN.to_owned(),
            moves,
            depth: STUDY_DEPTH,
            token,
        });
    }

    fn drain_engine(self: &Rc<Self>) {
        let Some(worker) = &self.worker else {
            return;
        };
        let mut budget = 32;
        while let Some(reply) = worker.poll() {
            budget -= 1;
            if budget < 0 {
                break;
            }
            match reply {
                Reply::Evaluation { analysis, token } => {
                    self.busy.set(false);
                    if token == self.token.get() {
                        self.show_evaluation(&analysis);
                    }
                    if self.pending.get() {
                        self.request_evaluation();
                    }
                }
                Reply::Failed(message) => {
                    self.busy.set(false);
                    self.variation.set_label(&format!("Engine: {message}"));
                }
                _ => {}
            }
        }
    }

    fn show_evaluation(&self, analysis: &Analysis) {
        let Some(position) = self.walk.borrow().as_ref().map(|w| w.position().clone()) else {
            return;
        };
        let mover = position.turn();

        if let Some(score) = analysis.score {
            self.evaluation.set_label(&describe_score(score, mover));
        }
        if let Some(best) = &analysis.best_move {
            let san = to_san(&position, std::slice::from_ref(best));
            let played = self
                .walk
                .borrow()
                .as_ref()
                .and_then(|w| w.next_san().map(str::to_owned));
            self.best.set_label(&match played {
                Some(played) if san.first().map(String::as_str) == Some(played.as_str()) => {
                    format!("{played} — the engine's choice too.")
                }
                Some(played) => format!(
                    "Played: {played}    Engine prefers: {}",
                    san.first().cloned().unwrap_or_default()
                ),
                None => format!(
                    "Engine prefers: {}",
                    san.first().cloned().unwrap_or_default()
                ),
            });
        }
        let line = to_san(&position, &analysis.pv);
        self.variation.set_label(&if line.is_empty() {
            String::new()
        } else {
            format!("depth {} · {}", analysis.depth, line.join(" "))
        });
    }
}

#[derive(Clone, Copy)]
enum Step {
    Start,
    Back,
    Forward,
    End,
}

/// Turn engine moves into notation a person reads.
fn to_san(position: &Chess, moves: &[String]) -> Vec<String> {
    let mut position = position.clone();
    let mut out = Vec::with_capacity(moves.len());
    for uci in moves {
        let Ok(parsed) = uci.parse::<UciMove>() else {
            break;
        };
        let Ok(mv) = parsed.to_move(&position) else {
            break;
        };
        out.push(San::from_move(&position, mv).to_string());
        position.play_unchecked(mv);
    }
    out
}

/// State the evaluation from White's point of view, as every chess book does,
/// rather than from whoever happens to be moving.
fn describe_score(score: Score, mover: shakmaty::Color) -> String {
    let flip = |v: f64| {
        if mover == shakmaty::Color::White {
            v
        } else {
            -v
        }
    };
    match score {
        Score::Mate(n) => {
            let n = if mover == shakmaty::Color::White {
                n
            } else {
                -n
            };
            if n >= 0 {
                format!("Mate in {n} for White")
            } else {
                format!("Mate in {} for Black", -n)
            }
        }
        Score::Cp(cp) => {
            let pawns = flip(f64::from(cp) / 100.0);
            if pawns.abs() < 0.15 {
                "Level".to_owned()
            } else {
                format!("{pawns:+.2}")
            }
        }
    }
}

/// The scoresheet, with the current point marked.
fn format_moves(san: &[String], index: usize) -> String {
    let mut out = String::new();
    for (i, chunk) in san.chunks(2).enumerate() {
        out.push_str(&format!("{}. {}", i + 1, chunk[0]));
        if let Some(black) = chunk.get(1) {
            out.push_str(&format!(" {black}"));
        }
        out.push('\n');
    }
    if index == 0 {
        out.push_str("(start)");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use shakmaty::Color;

    #[test]
    fn evaluations_are_stated_from_whites_point_of_view() {
        // Plus two pawns for the side to move; White to move means +2.00.
        assert_eq!(describe_score(Score::Cp(200), Color::White), "+2.00");
        // The same score with Black to move is minus two for White.
        assert_eq!(describe_score(Score::Cp(200), Color::Black), "-2.00");
    }

    #[test]
    fn a_near_equal_position_is_called_level_rather_than_a_number() {
        assert_eq!(describe_score(Score::Cp(10), Color::White), "Level");
        assert_eq!(describe_score(Score::Cp(-10), Color::Black), "Level");
    }

    #[test]
    fn mate_names_the_side_delivering_it() {
        assert_eq!(
            describe_score(Score::Mate(3), Color::White),
            "Mate in 3 for White"
        );
        assert_eq!(
            describe_score(Score::Mate(3), Color::Black),
            "Mate in 3 for Black"
        );
        assert_eq!(
            describe_score(Score::Mate(-2), Color::White),
            "Mate in 2 for Black"
        );
    }

    #[test]
    fn engine_moves_become_readable_notation() {
        let line: Vec<String> = ["e2e4", "e7e5", "g1f3"]
            .iter()
            .map(|m| m.to_string())
            .collect();
        assert_eq!(to_san(&Chess::default(), &line), ["e4", "e5", "Nf3"]);
    }

    #[test]
    fn an_unplayable_variation_is_truncated_rather_than_dropped() {
        let line: Vec<String> = ["e2e4", "a1a8"].iter().map(|m| m.to_string()).collect();
        assert_eq!(to_san(&Chess::default(), &line), ["e4"]);
    }

    #[test]
    fn the_scoresheet_pairs_moves_by_number() {
        let san: Vec<String> = ["e4", "e5", "Nf3"].iter().map(|m| m.to_string()).collect();
        let text = format_moves(&san, 3);
        assert!(text.contains("1. e4 e5"));
        assert!(text.contains("2. Nf3"));
    }
}
