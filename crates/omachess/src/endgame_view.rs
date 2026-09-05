//! Converting theoretical endgames against the best defence the engine has.
//!
//! Everything else here measures against something arguable. This does not: the
//! position is a win or it is not, the tablebase said which, and either you
//! converted it or you did not. It is the plainest evidence of skill the
//! application can produce.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk4::prelude::*;
use gtk4::{
    glib, Align, AspectFrame, Box as GtkBox, Button, DropDown, Label, Orientation, StringList,
};
use omachess_core::endgame::{self, Endgame, Objective, Outcome, ENDGAMES};
use omachess_core::game::{find_move, Game};
use omachess_core::store::Store;
use shakmaty::{Color, Position, Square};

use crate::board::BoardView;
use crate::engine_worker::{EngineWorker, Reply, Request};
use crate::pieces::PieceSet;
use crate::sound::{Cue, Sounds};

/// How long the engine gets for each defensive move. Long enough that the
/// defence is genuinely hard, short enough that a session is not a wait.
const DEFENCE_MS: u64 = 700;
const POLL_MS: u32 = 80;

pub struct EndgameView {
    root: GtkBox,
    board: Rc<BoardView>,
    sounds: Rc<Sounds>,
    store: Rc<RefCell<Store>>,
    worker: Option<EngineWorker>,
    game: RefCell<Option<Game>>,
    /// Which entry of `ENDGAMES` is loaded.
    current: Cell<usize>,
    picker: DropDown,
    title: Label,
    objective: Label,
    idea: Label,
    status: Label,
    record: Label,
    countdown: Label,
    start: Button,
    thinking: Cell<bool>,
    /// Set once the attempt has been judged, so it is recorded exactly once.
    settled: Cell<bool>,
    /// The sitting these attempts belong to.
    sitting: Cell<Option<i64>>,
    /// When the move being considered began.
    move_started: Cell<Option<std::time::Instant>>,
}

impl EndgameView {
    pub fn new(
        store: Rc<RefCell<Store>>,
        pieces: Option<Rc<PieceSet>>,
        sounds: Rc<Sounds>,
        engine: Option<std::path::PathBuf>,
    ) -> Rc<Self> {
        let board = BoardView::new(pieces);
        let worker = engine.map(EngineWorker::spawn);

        let names: Vec<&str> = ENDGAMES.iter().map(|e| e.name).collect();
        let picker = DropDown::new(Some(StringList::new(&names)), gtk4::Expression::NONE);
        picker.set_valign(Align::Center);

        let title = Label::builder().halign(Align::Start).wrap(true).build();
        title.add_css_class("title-4");

        let objective = Label::builder().halign(Align::Start).build();
        objective.add_css_class("omachess-status");

        let idea = Label::builder()
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(38)
            .build();
        idea.add_css_class("dim-label");

        let status = Label::builder()
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(38)
            .build();

        let record = Label::builder().halign(Align::Start).build();
        record.add_css_class("dim-label");

        let countdown = Label::builder().halign(Align::Start).build();
        countdown.add_css_class("dim-label");

        let start = Button::with_label("Begin");
        start.add_css_class("suggested-action");

        // Stacked, not side by side. The picker's label is a whole sentence —
        // a date, a phase and a cost — and next to it the Begin button was
        // pushed off the edge of the window, which made the entire tab
        // unusable: the board was there and nothing could start it.
        let controls = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(8)
            .build();
        picker.set_hexpand(true);
        start.set_halign(Align::Start);
        controls.append(&picker);
        controls.append(&start);

        let panel = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(10)
            .build();
        panel.set_width_request(320);
        panel.append(&title);
        panel.append(&objective);
        panel.append(&idea);
        panel.append(&controls);
        panel.append(&status);
        panel.append(&countdown);
        panel.append(&record);

        let frame = AspectFrame::builder()
            .ratio(1.0)
            .obey_child(false)
            .hexpand(true)
            .vexpand(true)
            .build();
        frame.set_child(Some(board.widget()));

        let root = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(18)
            .build();
        root.append(&frame);
        root.append(&panel);

        let view = Rc::new(Self {
            root,
            board,
            sounds,
            store,
            worker,
            game: RefCell::new(None),
            current: Cell::new(0),
            picker,
            title,
            objective,
            idea,
            status,
            record,
            countdown,
            start,
            thinking: Cell::new(false),
            settled: Cell::new(true),
            sitting: Cell::new(None),
            move_started: Cell::new(None),
        });

        view.describe(0);

        let weak: Weak<Self> = Rc::downgrade(&view);
        view.picker.connect_selected_notify(move |picker| {
            if let Some(view) = weak.upgrade() {
                view.describe(picker.selected() as usize);
            }
        });

        let weak: Weak<Self> = Rc::downgrade(&view);
        view.start.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.begin();
            }
        });

        let weak: Weak<Self> = Rc::downgrade(&view);
        view.board.connect_drag(move |from, to| {
            if let Some(view) = weak.upgrade() {
                view.play(from, to);
            }
        });

        // Clicking a piece and then its destination has to work too. Wiring
        // only the drag left this board silently ignoring half the ways a
        // person moves a piece, with nothing on screen to say why.
        let weak: Weak<Self> = Rc::downgrade(&view);
        view.board.connect_move(move |square| {
            if let Some(view) = weak.upgrade() {
                view.on_square(square);
            }
        });

        if view.worker.is_some() {
            let weak: Weak<Self> = Rc::downgrade(&view);
            glib::timeout_add_local(
                std::time::Duration::from_millis(POLL_MS as u64),
                move || match weak.upgrade() {
                    Some(view) => {
                        view.collect();
                        glib::ControlFlow::Continue
                    }
                    None => glib::ControlFlow::Break,
                },
            );
        }

        view
    }

    pub fn widget(&self) -> &GtkBox {
        &self.root
    }

    fn entry(&self) -> &'static Endgame {
        &ENDGAMES[self.current.get().min(ENDGAMES.len() - 1)]
    }

    /// Show what an endgame asks before it is started.
    fn describe(&self, index: usize) {
        self.current.set(index.min(ENDGAMES.len() - 1));
        let entry = self.entry();
        self.title.set_label(entry.name);
        self.objective.set_label(&match entry.objective {
            Objective::Win => match entry.dtm {
                Some(dtm) => format!("Win it — mate in {dtm} with best play"),
                None => "Win it".to_owned(),
            },
            Objective::Draw => "Hold the draw".to_owned(),
        });
        self.idea.set_label(entry.idea);
        if let Some(position) = entry.position() {
            self.board.set_position(&position);
            self.board.set_orientation(Color::White);
        }
        self.status.set_label("");
        self.countdown.set_label("");
        self.show_record();
    }

    fn show_record(&self) {
        let entry = self.entry();
        let (attempts, achieved) = self
            .store
            .borrow()
            .endgame_record(entry.key)
            .unwrap_or((0, 0));
        self.record.set_label(&if attempts == 0 {
            "Not attempted yet.".to_owned()
        } else {
            format!(
                "Converted {achieved} of {attempts} attempt{}.",
                if attempts == 1 { "" } else { "s" }
            )
        });
    }

    fn begin(&self) {
        let entry = self.entry();
        let Some(game) = Game::from_fen(Color::White, entry.fen) else {
            self.status.set_label("This position could not be set up.");
            return;
        };
        self.board.set_position(game.position());
        self.board.set_orientation(Color::White);
        self.board.set_last_move(None);
        self.board.set_mate(None);
        if self.sitting.get().is_none() {
            match self
                .store
                .borrow()
                .begin_session("endgame", chrono::Utc::now())
            {
                Ok(id) => self.sitting.set(Some(id)),
                Err(e) => omachess_core::diagnostics::record_error("endgame::begin_session", e),
            }
        }
        crate::announce::clear();
        self.move_started.set(Some(std::time::Instant::now()));
        *self.game.borrow_mut() = Some(game);
        self.settled.set(false);
        self.thinking.set(false);
        self.status
            .set_label("Your move. The engine defends at full strength.");
        self.update_countdown();
    }

    fn update_countdown(&self) {
        let game = self.game.borrow();
        let Some(game) = game.as_ref() else {
            self.countdown.set_label("");
            return;
        };
        let left = endgame::moves_until_fifty(game.position());
        self.countdown.set_label(&match self.entry().objective {
            Objective::Win => format!("{left} moves before the fifty-move draw."),
            Objective::Draw => format!("{left} moves to survive."),
        });
    }

    /// Clicking a piece and then where it should go.
    fn on_square(self: &Rc<Self>, square: Square) {
        if self.thinking.get() || self.settled.get() {
            return;
        }
        let Some(from) = self.board.selected() else {
            // Only own pieces can be picked up, or the first click selects an
            // empty square and the second looks like it did nothing.
            if self.has_own_piece(square) {
                self.board.select(Some(square));
            }
            return;
        };
        self.board.select(None);
        if from != square {
            self.play(from, square);
        }
    }

    /// Whether the player owns the piece on this square.
    fn has_own_piece(&self, square: Square) -> bool {
        let game = self.game.borrow();
        let Some(game) = game.as_ref() else {
            return false;
        };
        game.position().board().color_at(square) == Some(game.player())
    }

    /// The player's move, asking first when a pawn reaches the last rank.
    fn play(self: &Rc<Self>, from: Square, to: Square) {
        if self.thinking.get() || self.settled.get() {
            return;
        }
        let choices = {
            let game = self.game.borrow();
            match game.as_ref() {
                Some(game) => omachess_core::game::promotion_choices(game.position(), from, to),
                None => Vec::new(),
            }
        };
        if choices.len() > 1 {
            let white = self
                .game
                .borrow()
                .as_ref()
                .map(|game| game.player() == Color::White)
                .unwrap_or(true);
            let view = self.clone();
            self.board.ask_promotion(to, white, &choices, move |role| {
                view.play_promoting(from, to, Some(role));
            });
            return;
        }
        self.play_promoting(from, to, None);
    }

    fn play_promoting(&self, from: Square, to: Square, promotion: Option<shakmaty::Role>) {
        let mut slot = self.game.borrow_mut();
        let Some(game) = slot.as_mut() else {
            return;
        };
        // The player is always White in these, but naming the side rather than
        // assuming it is what the drill board got wrong.
        if game.position().turn() != game.player() {
            return;
        }
        let prefer = promotion.map(|role| {
            format!(
                "{from}{to}{}",
                match role {
                    shakmaty::Role::Rook => "r",
                    shakmaty::Role::Bishop => "b",
                    shakmaty::Role::Knight => "n",
                    _ => "q",
                }
            )
        });
        let Some(mv) = find_move(game.position(), from, to, prefer.as_deref()) else {
            crate::announce::say(
                crate::announce::Tone::Rejected,
                &format!("{from} to {to} is not a legal move here"),
            );
            return;
        };
        let capture = game.position().board().occupied().contains(to);
        if game.play(&mv).is_err() {
            crate::announce::say(
                crate::announce::Tone::Rejected,
                &format!("{from} to {to} cannot be played in this position"),
            );
            return;
        }
        self.board.set_position(game.position());
        self.board.set_last_move(Some((from, to)));
        self.sounds
            .play(if capture { Cue::Capture } else { Cue::Move });
        drop(slot);

        {
            let thinking = self
                .move_started
                .replace(Some(std::time::Instant::now()))
                .map(|start| start.elapsed())
                .unwrap_or_default();
            let (ply, uci, left) = {
                let guard = self.game.borrow();
                match guard.as_ref() {
                    Some(game) => (
                        game.moves().len().saturating_sub(1) as u32,
                        game.moves().last().cloned().unwrap_or_default(),
                        endgame::moves_until_fifty(game.position()),
                    ),
                    None => (0, String::new(), 0),
                }
            };
            // The fifty-move budget is the thing under pressure in a conversion,
            // so how much of it was left is what makes the move readable later.
            let detail = format!("{{\"moves_until_fifty\":{left}}}");
            if let Err(e) = self.store.borrow().log_move(
                self.sitting.get(),
                "endgame",
                self.entry().key,
                chrono::Utc::now(),
                ply,
                &uci,
                thinking,
                &detail,
            ) {
                omachess_core::diagnostics::record_error("endgame::log_move", e);
            }
        }

        self.update_countdown();
        if self.judge() {
            return;
        }
        self.ask_engine();
    }

    /// Judge the position if it has finished. Returns whether it had.
    fn judge(&self) -> bool {
        let (finished, moves) = {
            let game = self.game.borrow();
            let Some(game) = game.as_ref() else {
                return false;
            };
            (
                endgame::conclusion(game.position()),
                game.moves().len() as u32,
            )
        };
        let Some(winner) = finished else {
            return false;
        };
        if self.settled.replace(true) {
            return true;
        }

        let entry = self.entry();
        let outcome = entry.judge(winner);
        let achieved = outcome == Outcome::Achieved;
        if let Err(e) =
            self.store
                .borrow()
                .record_endgame(entry.key, chrono::Utc::now(), achieved, moves)
        {
            omachess_core::diagnostics::record_error("endgame::record_endgame", e);
        }

        let verdict = match (entry.objective, winner, achieved) {
            (Objective::Win, Some(Color::White), _) => "Converted.".to_owned(),
            (Objective::Win, None, _) => {
                "Drawn — the win was there and it got away. Try it again.".to_owned()
            }
            (Objective::Win, Some(_), _) => "Lost a won position.".to_owned(),
            (Objective::Draw, None, _) => "Held.".to_owned(),
            (Objective::Draw, Some(Color::White), _) => {
                "Won a position that was only level.".to_owned()
            }
            (Objective::Draw, Some(_), _) => {
                "Lost a position that was a draw. That is the one to study.".to_owned()
            }
        };
        self.status.set_label(&verdict);
        crate::announce::say(
            if achieved {
                crate::announce::Tone::Won
            } else {
                crate::announce::Tone::Lost
            },
            &verdict,
        );
        self.countdown.set_label("");
        self.show_record();
        true
    }

    fn ask_engine(&self) {
        let Some(worker) = self.worker.as_ref() else {
            self.status
                .set_label("No engine, so there is nothing to play against.");
            return;
        };
        let (fen, moves) = {
            let game = self.game.borrow();
            let Some(game) = game.as_ref() else {
                return;
            };
            (game.initial_fen().to_owned(), game.moves().to_vec())
        };
        self.thinking.set(true);
        worker.send(Request::BestMove {
            fen,
            moves,
            millis: DEFENCE_MS,
        });
    }

    fn collect(&self) {
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        while let Some(reply) = worker.poll() {
            match reply {
                Reply::Move(uci) => self.apply_engine_move(&uci),
                Reply::Failed(why) => {
                    self.thinking.set(false);
                    self.status.set_label(&format!("The engine stopped: {why}"));
                }
                _ => {}
            }
        }
    }

    fn apply_engine_move(&self, uci: &str) {
        self.thinking.set(false);
        let mut slot = self.game.borrow_mut();
        let Some(game) = slot.as_mut() else {
            return;
        };
        let Ok(parsed) = uci.parse::<shakmaty::uci::UciMove>() else {
            return;
        };
        let Ok(mv) = parsed.to_move(game.position()) else {
            return;
        };
        let capture = mv.is_capture();
        let (from, to) = (mv.from(), Some(mv.to()));
        if game.play(&mv).is_err() {
            self.status
                .set_label("The engine offered a move that is not legal here.");
            return;
        }
        self.board.set_position(game.position());
        if let (Some(from), Some(to)) = (from, to) {
            self.board.set_last_move(Some((from, to)));
        }
        self.sounds
            .play(if capture { Cue::Capture } else { Cue::Move });
        drop(slot);

        self.update_countdown();
        self.judge();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omachess_core::endgame::{conclusion, find, moves_until_fifty};
    use omachess_core::game::Game;
    use shakmaty::fen::Fen;
    use shakmaty::CastlingMode;

    fn position(fen: &str) -> shakmaty::Chess {
        fen.parse::<Fen>()
            .unwrap()
            .into_position(CastlingMode::Standard)
            .unwrap()
    }

    /// Every entry has to be loadable as a game the player can move in, or the
    /// picker would offer a position that cannot be started.
    #[test]
    fn every_entry_loads_as_a_playable_game() {
        for entry in ENDGAMES {
            let game = Game::from_fen(Color::White, entry.fen)
                .unwrap_or_else(|| panic!("{} would not load", entry.key));
            assert_eq!(game.position().turn(), Color::White);
            assert!(
                !game.position().legal_moves().is_empty(),
                "{}: nothing to play",
                entry.key
            );
        }
    }

    /// The status line has to distinguish holding a draw from failing to win,
    /// because they are the same result and opposite outcomes.
    #[test]
    fn the_same_result_reads_differently_against_different_objectives() {
        let win = find("lucena").unwrap();
        let hold = find("kp-drawn").unwrap();

        assert_eq!(
            win.judge(None),
            Outcome::Failed,
            "a draw loses a won position"
        );
        assert_eq!(
            hold.judge(None),
            Outcome::Achieved,
            "a draw holds a level one"
        );
    }

    /// The countdown is what the defender is playing towards and the attacker
    /// is racing, so it has to fall as moves are made.
    #[test]
    fn the_fifty_move_countdown_falls() {
        let fresh = position(find("lucena").unwrap().fen);
        assert_eq!(moves_until_fifty(&fresh), 50);

        let late = position("8/8/8/4k3/8/8/8/1NB1K3 w - - 90 60");
        assert_eq!(moves_until_fifty(&late), 5);
        assert_eq!(conclusion(&late), None, "still playable");
    }
}
