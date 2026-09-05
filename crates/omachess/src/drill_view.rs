//! Playing on from the positions you lost from.
//!
//! A puzzle asks for one move and then tells you whether you found it. Losing a
//! game does not feel like that and fixing one does not work like that: the
//! move is where the work starts, not where it ends. What actually went wrong
//! is everything after it.
//!
//! So this hands the position back exactly as it stood before the mistake, says
//! what it was worth, and asks for the result — against an engine still trying
//! to beat you. Find the move and then convert it, or discover that you cannot,
//! which is the more useful of the two answers.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk4::prelude::*;
use gtk4::{
    glib, Align, AspectFrame, Box as GtkBox, Button, DropDown, Label, Orientation, StringList,
};
use omachess_core::endgame::{self, Objective};
use omachess_core::game::{find_move, Game};
use omachess_core::playout;
use omachess_core::store::DrillOrigin;
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

pub struct DrillView {
    root: GtkBox,
    board: Rc<BoardView>,
    sounds: Rc<Sounds>,
    store: Rc<RefCell<Store>>,
    worker: Option<EngineWorker>,
    game: RefCell<Option<Game>>,
    /// The positions on offer, worst first, and which one is loaded.
    positions: RefCell<Vec<(String, DrillOrigin, String)>>,
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

impl DrillView {
    pub fn new(
        store: Rc<RefCell<Store>>,
        pieces: Option<Rc<PieceSet>>,
        sounds: Rc<Sounds>,
        engine: Option<std::path::PathBuf>,
    ) -> Rc<Self> {
        let board = BoardView::new(pieces);
        let worker = engine.map(EngineWorker::spawn);

        // Filled in when the view is shown: the positions worth replaying
        // change every time a game is imported or played.
        let picker = DropDown::new(Some(StringList::new(&[])), gtk4::Expression::NONE);
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
            positions: RefCell::new(Vec::new()),
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

        view.reload();

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

    /// How many moves the position in progress has seen, so a test can tell
    /// whether a click actually did anything.
    pub(crate) fn moves_played(&self) -> usize {
        self.game
            .borrow()
            .as_ref()
            .map_or(0, |game| game.moves().len())
    }

    /// Enough of the internal state to say where a click went wrong.
    pub(crate) fn describe_state(&self) -> String {
        let positions = self.positions.borrow().len();
        let game = self.game.borrow();
        match game.as_ref() {
            None => format!(
                "{positions} positions loaded, no game started (settled={})",
                self.settled.get()
            ),
            Some(game) => format!(
                "{positions} positions loaded, playing as {:?}, {} moves in, \
                 turn={:?}, settled={}, thinking={}",
                game.player(),
                game.moves().len(),
                game.position().turn(),
                self.settled.get(),
                self.thinking.get()
            ),
        }
    }

    /// The board itself, so a test can click it the way a person does.
    pub(crate) fn board(&self) -> &Rc<BoardView> {
        &self.board
    }

    pub fn widget(&self) -> &GtkBox {
        &self.root
    }

    /// The position now selected: its id, where it came from, and the FEN it
    /// is played out from.
    fn entry(&self) -> Option<(String, DrillOrigin, String)> {
        self.positions.borrow().get(self.current.get()).cloned()
    }

    /// Refresh the list of positions worth replaying.
    ///
    /// Called when the view is shown, because importing a game or losing a new
    /// one changes what is worth working on.
    pub fn reload(&self) {
        let found = {
            let store = self.store.borrow();
            store.drills_to_play(40).unwrap_or_default()
        };
        let mut rows = Vec::new();
        let mut labels = Vec::new();
        for (id, origin) in found {
            // The position is the puzzle's own, which is where the mistake was
            // made rather than after it.
            let Some(puzzle) = self.store.borrow().puzzle(&id).ok().flatten() else {
                continue;
            };
            let Some(fen) = omachess_core::playout::position_to_play(&puzzle) else {
                continue;
            };
            labels.push(format!(
                "{} {} — cost {:.0}%",
                origin.played_at.format("%-d %b"),
                origin.phase,
                origin.lost * 100.0
            ));
            rows.push((id, origin, fen));
        }
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        self.picker.set_model(Some(&StringList::new(&refs)));
        *self.positions.borrow_mut() = rows;
        self.current.set(0);
        if self.positions.borrow().is_empty() {
            self.title.set_label("Nothing to replay yet");
            self.idea.set_label(
                "Import a PGN export or play a game here. The positions you lost from \
                 collect up and come back as drills.",
            );
            self.objective.set_label("");
            self.record.set_label("");
            // Something rather than a blank grid, which reads as broken.
            self.board.set_orientation(Color::White);
            self.board.set_position(&shakmaty::Chess::default());
        } else {
            self.picker.set_selected(0);
            self.describe(0);
        }
    }

    /// Show what a position asks before it is started.
    fn describe(&self, index: usize) {
        let count = self.positions.borrow().len();
        if count == 0 {
            return;
        }
        self.current.set(index.min(count - 1));
        let Some((_, origin, fen)) = self.entry() else {
            return;
        };
        self.title
            .set_label(&format!("Your game, move {}", origin.ply / 2 + 1));
        self.objective
            .set_label(match playout::objective_for(origin.win_before) {
                Objective::Win => "You were winning here. Win it.",
                Objective::Draw => "Save it — a draw is a pass.",
            });
        // The brief names the move that was played, so it is withheld until the
        // position has been played out: the point is to walk in blind.
        self.idea
            .set_label("Play it out against the engine. What you played last time comes after.");
        if let Some(game) = Game::from_fen(self.player_side(&fen), &fen) {
            self.board.set_position(game.position());
            self.board.set_orientation(self.player_side(&fen));
        }
        self.status.set_label("");
        self.countdown.set_label("");
        self.show_record();
    }

    /// Whichever side is to move in the position is the side the player had.
    fn player_side(&self, fen: &str) -> Color {
        Game::from_fen(Color::White, fen)
            .map(|game| game.position().turn())
            .unwrap_or(Color::White)
    }

    fn show_record(&self) {
        let (attempts, achieved, retired) = {
            let store = self.store.borrow();
            (
                store.drill_playout_record().unwrap_or((0, 0)).0,
                store.drill_playout_record().unwrap_or((0, 0)).1,
                store.retired_drill_count().unwrap_or(0),
            )
        };
        let waiting = self.positions.borrow().len();

        // Said out loud: a position quietly vanishing from the list looks like
        // a bug rather than like progress.
        let mastered = if retired == 0 {
            String::new()
        } else {
            format!(
                " {retired} mastered and set aside — won twice at least {:.0}h apart, \
                 most recently.",
                omachess_core::store::MIN_REPEAT_HOURS
            )
        };
        self.record.set_label(&if attempts == 0 {
            format!("{waiting} positions waiting. None played out yet.{mastered}")
        } else {
            format!("{achieved} of {attempts} played out successfully.{mastered}")
        });
    }

    pub(crate) fn begin(&self) {
        let Some((_, _, fen)) = self.entry() else {
            self.status.set_label("Nothing to play.");
            return;
        };
        let side = self.player_side(&fen);
        let Some(game) = Game::from_fen(side, &fen) else {
            self.status.set_label("This position could not be set up.");
            return;
        };
        self.board.set_position(game.position());
        self.board.set_orientation(side);
        self.board.set_last_move(None);
        self.board.set_mate(None);
        if self.sitting.get().is_none() {
            match self
                .store
                .borrow()
                .begin_session("drill", chrono::Utc::now())
            {
                Ok(id) => self.sitting.set(Some(id)),
                Err(e) => omachess_core::diagnostics::record_error("drill::begin_session", e),
            }
        }
        crate::announce::clear();
        self.move_started.set(Some(std::time::Instant::now()));
        *self.game.borrow_mut() = Some(game);
        self.settled.set(false);
        self.thinking.set(false);
        self.status.set_label("Your move. Play the position out.");
        self.update_countdown();
    }

    fn update_countdown(&self) {
        let game = self.game.borrow();
        let Some(game) = game.as_ref() else {
            self.countdown.set_label("");
            return;
        };
        // A middlegame played out has no fifty-move deadline worth watching,
        // so what is shown is simply how far this attempt has run.
        let played = game.moves().len() / 2 + 1;
        self.countdown.set_label(&format!("move {played}"));
    }

    /// Clicking a piece and then where it should go.
    pub(crate) fn on_square(self: &Rc<Self>, square: Square) {
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
            crate::promotion::ask(self.board.widget(), &choices, white, move |role| {
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
        // Not White — whichever side made the mistake. This was copied from the
        // endgame board, where the player is always White, and it silently
        // discarded every move in every drill taken from a game played as
        // Black, which is most of them.
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
            // Which position of the player's own this move belongs to, so a
            // replay can be read back against the game it came from.
            let detail = format!("{{\"moves_until_fifty\":{left}}}");
            let subject = self.entry().map(|(id, _, _)| id).unwrap_or_default();
            if let Err(e) = self.store.borrow().log_move(
                self.sitting.get(),
                "drill",
                &subject,
                chrono::Utc::now(),
                ply,
                &uci,
                thinking,
                &detail,
            ) {
                omachess_core::diagnostics::record_error("drill::log_move", e);
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
        let (finished, moves, side) = {
            let game = self.game.borrow();
            let Some(game) = game.as_ref() else {
                return false;
            };
            (
                endgame::conclusion(game.position()),
                game.moves().len() as u32,
                game.player(),
            )
        };
        let Some(winner) = finished else {
            return false;
        };
        if self.settled.replace(true) {
            return true;
        }
        let Some((id, origin, _)) = self.entry() else {
            return true;
        };

        let objective = playout::objective_for(origin.win_before);
        let achieved = playout::judge(objective, side, winner);
        let result = match winner {
            Some(colour) if colour == side => "won",
            Some(_) => "lost",
            None => "drawn",
        };
        if let Err(e) = self.store.borrow().record_drill_attempt(
            &id,
            chrono::Utc::now(),
            achieved,
            moves,
            result,
        ) {
            omachess_core::diagnostics::record_error("drill::record_attempt", e);
        }

        let verdict = match (objective, achieved) {
            (Objective::Win, true) => "Won it this time.",
            (Objective::Win, false) => "The win got away again. That is the position to work on.",
            (Objective::Draw, true) => "Saved.",
            (Objective::Draw, false) => "Lost it again from here.",
        };
        self.status.set_label(verdict);
        crate::announce::say(
            if achieved {
                crate::announce::Tone::Won
            } else {
                crate::announce::Tone::Lost
            },
            verdict,
        );
        // Only now: what was played the first time, and what the engine wanted.
        // Withholding it until the position has been fought through is the
        // whole point of walking in blind.
        self.idea.set_label(&format!(
            "{} The engine wanted {}.",
            playout::brief(&origin),
            origin.best
        ));
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
