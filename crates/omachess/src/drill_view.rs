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

/// How long the engine gets for each defensive move. Long enough that the
/// defence is genuinely hard, short enough that a session is not a wait.
const DEFENCE_MS: u64 = 700;
const POLL_MS: u32 = 80;

/// The position every ordinary game starts from.
const START_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

/// A FEN as a position. `drill::position_after` needs a move to apply and
/// returns nothing without one, which is not what is wanted here.
fn position_of(fen: &str) -> Option<shakmaty::Chess> {
    fen.parse::<shakmaty::fen::Fen>()
        .ok()?
        .into_position::<shakmaty::Chess>(shakmaty::CastlingMode::Standard)
        .ok()
}

/// Whether two positions are the same to play from, ignoring the clocks.
fn same_position(a: &shakmaty::Chess, b: &shakmaty::Chess) -> bool {
    let strip = |p: &shakmaty::Chess| {
        shakmaty::fen::Fen::from_position(p, shakmaty::EnPassantMode::Legal)
            .to_string()
            .split_whitespace()
            .take(4)
            .collect::<Vec<_>>()
            .join(" ")
    };
    strip(a) == strip(b)
}

/// Play a line out, or nothing if any of it will not play.
fn replay(from: &shakmaty::Chess, moves: &[String]) -> Option<shakmaty::Chess> {
    let mut position = from.clone();
    for uci in moves {
        let mv = uci.parse::<shakmaty::uci::UciMove>().ok()?.to_move(&position).ok()?;
        position.play_unchecked(mv);
    }
    Some(position)
}

/// What the exercise is asking for right now.
///
/// It used to ask for only one thing — play the position out — and told the
/// solver nothing about it first: "Play it out against the engine. What you
/// played last time comes after." Walking in blind is a fine way to test
/// somebody and a poor way to teach them, and it is not what going back over
/// your own game is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// What you played, what it cost, and what should have been played.
    Find,
    /// Now convert the position you just found.
    PlayOut,
}

pub struct DrillView {
    root: GtkBox,
    board: Rc<BoardView>,
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
    /// Which half of the exercise is running.
    stage: Cell<Stage>,
    /// Wrong answers to the "find it" question, before the answer is given.
    misses: Cell<u32>,
    /// Why the last attempt was wrong, once the engine has said.
    lesson: Label,
    /// One axis to walk: the moves that led to the mistake, then — once it has
    /// been answered — the line that should have been played. The cursor starts
    /// at the mistake, so Back is how the position arose and Next is what to do
    /// about it.
    nav_base: RefCell<String>,
    nav_moves: RefCell<Vec<String>>,
    /// Where in `nav_moves` the mistake sits.
    nav_pivot: Cell<usize>,
    nav_at: Cell<usize>,
    prev: Button,
    next: Button,
    /// One question outstanding, and answers for a position already left
    /// behind are dropped rather than shown against the wrong board.
    lesson_busy: Cell<bool>,
    lesson_token: Cell<u64>,
    asked_about: RefCell<Option<(shakmaty::Chess, String)>>,
}

impl DrillView {
    pub fn new(
        store: Rc<RefCell<Store>>,
        pieces: Option<Rc<PieceSet>>,
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

        // Why the last attempt was wrong, in the engine's words. The tab used
        // to answer a wrong move with nothing at all.
        let lesson = Label::builder()
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(38)
            .build();
        lesson.add_css_class("omachess-lesson");

        // Stepping the engine's line once it is on the table.
        let prev = Button::with_label("‹ Back");
        let next = Button::with_label("Next ›");
        let steps = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(6)
            .build();
        steps.append(&prev);
        steps.append(&next);
        steps.set_visible(false);

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
        panel.append(&lesson);
        panel.append(&steps);
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
            stage: Cell::new(Stage::Find),
            misses: Cell::new(0),
            lesson,
            nav_base: RefCell::new(String::new()),
            nav_moves: RefCell::new(Vec::new()),
            nav_pivot: Cell::new(0),
            nav_at: Cell::new(0),
            prev,
            next,
            lesson_busy: Cell::new(false),
            lesson_token: Cell::new(0),
            asked_about: RefCell::new(None),
        });

        view.reload();

        let weak: Weak<Self> = Rc::downgrade(&view);
        view.picker.connect_selected_notify(move |picker| {
            if let Some(view) = weak.upgrade() {
                view.describe(picker.selected() as usize);
            }
        });

        let weak: Weak<Self> = Rc::downgrade(&view);
        view.prev.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.step_line(false);
            }
        });
        let weak: Weak<Self> = Rc::downgrade(&view);
        view.next.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.step_line(true);
            }
        });

        let weak: Weak<Self> = Rc::downgrade(&view);
        view.start.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                // One button, two jobs, because they are the two halves of one
                // exercise: give up on finding it, then convert what you found.
                if view.stage.get() == Stage::Find {
                    // Asking to be shown is not finding it.
                    view.record_answer(false);
                    view.reveal_answer();
                } else {
                    view.begin();
                }
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
    /// Open on a particular position, reloading if it is not on the list yet.
    ///
    /// Used by the Play tab when the player asks to practise the moment their
    /// game turned: the position was written to the store on the way here, so
    /// a reload will find it.
    pub(crate) fn focus(&self, puzzle_id: &str) {
        if !self.positions.borrow().iter().any(|(id, _, _)| id == puzzle_id) {
            self.reload();
        }
        let found = self
            .positions
            .borrow()
            .iter()
            .position(|(id, _, _)| id == puzzle_id);
        if let Some(index) = found {
            self.picker.set_selected(index as u32);
            self.describe(index);
        }
    }

    pub(crate) fn status_text(&self) -> String {
        self.status.text().to_string()
    }

    pub(crate) fn brief_text(&self) -> String {
        self.objective.text().to_string()
    }

    pub(crate) fn asked_text(&self) -> String {
        self.idea.text().to_string()
    }

    pub(crate) fn lesson_text(&self) -> String {
        self.lesson.text().to_string()
    }

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
            .set_label(&format!("Move {}", origin.ply / 2 + 1));
        // What you played and what it cost, said first. It used to be withheld
        // until after the position had been played out, on the reasoning that
        // the point was to walk in blind — which is a way to test somebody, not
        // to teach them, and is not what going back over your own game is for.
        self.objective.set_label(&format!(
            "{}??   -{:.0}%   you stood at {:.0}%",
            origin.played,
            origin.lost * 100.0,
            origin.win_before.max(0.0) * 100.0
        ));
        self.idea
            .set_label(match playout::objective_for(origin.win_before) {
                Objective::Win => "Winning was there. Find the move.",
                Objective::Draw => "A draw was there. Find the move.",
            });
        if let Some(game) = Game::from_fen(self.player_side(&fen), &fen) {
            self.board.set_position(game.position());
            self.board.set_orientation(self.player_side(&fen));
            *self.game.borrow_mut() = Some(game);
        }
        self.stage.set(Stage::Find);
        // The exercise is live as soon as it is on screen: the first half asks
        // a question, and a question you cannot answer until you press Begin is
        // not being asked.
        self.settled.set(false);
        self.thinking.set(false);
        self.misses.set(0);
        self.lesson.set_label("");
        self.build_navigation(&origin, &fen);
        self.start.set_label("Show me");
        self.status.set_label("");
        self.countdown.set_label("");
        self.show_record();
    }

    /// Note whether the move was found.
    ///
    /// Called exactly once per showing because answering — right, or wrong
    /// twice, or asking to be shown — always moves the exercise on to the
    /// play-out. A flag guarding against a second call was tried and removed:
    /// nothing could reach it, and a guard that cannot be made to fire is not
    /// protection, it is decoration that reads as protection.
    ///
    /// Kept apart from the play-out record because they are different
    /// questions. Seeing the move and failing to win with it, and grinding out
    /// a win from a position you never understood, both come out as one
    /// successful attempt otherwise — and the first is the half this teaches.
    fn record_answer(&self, found: bool) {
        let Some((id, _, _)) = self.entry() else {
            return;
        };
        if let Err(e) = self.store.borrow().record_drill_answer(
            &id,
            chrono::Utc::now(),
            found,
            self.misses.get(),
        ) {
            omachess_core::diagnostics::record_error("drill::record_answer", e);
        }
        self.show_record();
    }

    fn steps_visible(&self, on: bool) {
        if let Some(row) = self.prev.parent() {
            row.set_visible(on);
        }
    }

    /// Judge an answer to "find the move".
    ///
    /// Returns whether the move was the one the engine wanted.
    fn judge_answer(&self, mv: &shakmaty::Move) -> bool {
        let Some((_, origin, _)) = self.entry() else {
            return false;
        };
        let position = match self.game.borrow().as_ref() {
            Some(game) => game.position().clone(),
            None => return false,
        };
        let san = shakmaty::san::San::from_move(&position, *mv).to_string();
        san == origin.best
    }

    /// Ask the engine what punishes the move just tried.
    ///
    /// The same question the trainer asks, for the same reason: "not that one"
    /// tells the solver they failed and nothing about the board.
    fn ask_why(&self, mv: &shakmaty::Move) {
        let Some(worker) = &self.worker else {
            return;
        };
        if self.lesson_busy.get() {
            return;
        }
        let Some(position) = self.game.borrow().as_ref().map(|g| g.position().clone()) else {
            return;
        };
        let played = shakmaty::san::SanPlus::from_move(position.clone(), *mv).to_string();
        let mut after = position;
        after.play_unchecked(*mv);

        let token = self.lesson_token.get().wrapping_add(1);
        self.lesson_token.set(token);
        *self.asked_about.borrow_mut() = Some((after.clone(), played));

        let fen =
            shakmaty::fen::Fen::from_position(&after, shakmaty::EnPassantMode::Legal).to_string();
        if worker.send(Request::Evaluate {
            fen,
            moves: Vec::new(),
            depth: 18,
            token,
        }) {
            self.lesson_busy.set(true);
            self.lesson.set_label("Looking at why…");
        }
    }

    /// Put the answer on the table and let it be walked.
    fn reveal_answer(&self) {
        let Some((_, origin, _)) = self.entry() else {
            return;
        };
        let position = match self.game.borrow().as_ref() {
            Some(game) => game.position().clone(),
            None => return,
        };
        let line = omachess_core::game::line_to_san(&position, &origin.best_line);
        let shown = if line.is_empty() {
            origin.best.clone()
        } else {
            line.join("  ")
        };
        self.lesson
            .set_label(&format!("{} was the move. {shown}", origin.best));
        self.nav_moves
            .borrow_mut()
            .extend(origin.best_line.iter().cloned());
        self.steps_visible(self.nav_moves.borrow().len() > self.nav_pivot.get()
            || self.nav_pivot.get() > 0);
        self.stage.set(Stage::PlayOut);
        self.idea.set_label("Now play it out against the engine.");
        self.start.set_label("Play it out");
    }

    /// Step the engine's line on the board.
    /// Work out what can be walked, and put the cursor on the mistake.
    ///
    /// The moves that led here come from the game the position was taken out
    /// of. Games do not store the position they started from, so rather than
    /// assume the standard one, the moves are replayed from it and the result
    /// compared against the position this drill actually poses. If they agree,
    /// the context is real; if they do not — a game seeded from an opening
    /// line, or moves from some other game — there is simply no context to
    /// offer, which is far better than a board showing the wrong thing.
    fn build_navigation(&self, origin: &DrillOrigin, fen: &str) {
        self.nav_moves.borrow_mut().clear();
        *self.nav_base.borrow_mut() = fen.to_owned();
        self.nav_pivot.set(0);
        self.nav_at.set(0);
        self.steps_visible(false);

        let Some(game_id) = origin.game_id else {
            return;
        };
        let moves = self.store.borrow().game_moves(game_id).unwrap_or_default();
        let upto = origin.ply as usize;
        if moves.len() < upto {
            return;
        }
        let context: Vec<String> = moves[..upto].to_vec();

        let Some(start) = position_of(START_FEN) else {
            return;
        };
        let Some(reached) = replay(&start, &context) else {
            return;
        };
        let Some(posed) = position_of(fen) else {
            return;
        };
        // Pieces, side to move, castling and en passant — not the move counters.
        // A position reached by replaying is the same position whatever the
        // halfmove clock says, and comparing whole FENs made this fail on an
        // exact match.
        if !same_position(&reached, &posed) {
            return;
        }

        *self.nav_base.borrow_mut() = START_FEN.to_owned();
        *self.nav_moves.borrow_mut() = context;
        self.nav_pivot.set(upto);
        self.nav_at.set(upto);
        self.steps_visible(upto > 0);
    }

    /// Step the cursor along the axis and redraw.
    ///
    /// Forward cannot reach the answer while the question is open because the
    /// answer is not on the axis yet — `reveal_answer` appends it. A separate
    /// cap keyed on the stage was written first and removed: no mutation could
    /// make it fire, so it was decoration that read as a rule.
    fn step_line(&self, forward: bool) {
        let moves = self.nav_moves.borrow().clone();
        if moves.is_empty() {
            return;
        }
        let at = self.nav_at.get();
        let at = if forward {
            (at + 1).min(moves.len())
        } else {
            at.saturating_sub(1)
        };
        self.nav_at.set(at);

        let base = self.nav_base.borrow().clone();
        let Some(mut position) = position_of(&base) else {
            return;
        };
        let mut last = None;
        for uci in moves.iter().take(at) {
            let Ok(parsed) = uci.parse::<shakmaty::uci::UciMove>() else {
                break;
            };
            let Ok(mv) = parsed.to_move(&position) else {
                break;
            };
            last = mv.from().map(|from| (from, mv.to()));
            position.play_unchecked(mv);
        }
        self.board.set_position(&position);
        self.board.set_last_move(last);
        self.board.select(None);

        let pivot = self.nav_pivot.get();
        self.status.set_label(&match at.cmp(&pivot) {
            std::cmp::Ordering::Less => format!("{} moves before it", pivot - at),
            std::cmp::Ordering::Equal => "The position you had".to_owned(),
            std::cmp::Ordering::Greater => format!("Line: {} of {}", at - pivot, moves.len() - pivot),
        });
    }

    pub(crate) fn step_back(&self) {
        self.step_line(false);
    }

    pub(crate) fn step_forward(&self) {
        self.step_line(true);
    }

    /// Where the cursor is, and what it can reach. For the self-test.
    pub(crate) fn navigation(&self) -> (usize, usize, usize) {
        (
            self.nav_at.get(),
            self.nav_pivot.get(),
            self.nav_moves.borrow().len(),
        )
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
        let (asked, found) = self.store.borrow().drill_answer_record().unwrap_or((0, 0));
        let finding = if asked == 0 {
            String::new()
        } else {
            format!(" Move found {found} of {asked} times.")
        };
        self.record.set_label(&if attempts == 0 {
            format!("{waiting} positions waiting.{finding}{mastered}")
        } else {
            format!("{achieved} of {attempts} converted.{finding}{mastered}")
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
        // Begin is the second half: from here a move is a move again.
        self.stage.set(Stage::PlayOut);
        self.steps_visible(false);
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
            self.status
                .set_label(&format!("{from} to {to} is not legal here"));
            return;
        };

        // While the exercise is asking which move should have been played, a
        // move is an answer rather than a move: it is judged, explained, and
        // the position stays where it is.
        if self.stage.get() == Stage::Find {
            drop(slot);
            if self.judge_answer(&mv) {
                self.status.set_label("That is the move.");
                self.lesson.set_label("");
                self.record_answer(true);
                self.reveal_answer();
            } else {
                let misses = self.misses.get() + 1;
                self.misses.set(misses);
                if misses >= 2 {
                    self.status.set_label("Here it is.");
                    self.record_answer(false);
                    self.reveal_answer();
                } else {
                    self.status.set_label("Not that one.");
                    self.ask_why(&mv);
                }
            }
            return;
        }
        if game.play(&mv).is_err() {
            crate::announce::say(
                crate::announce::Tone::Rejected,
                &format!("{from} to {to} cannot be played in this position"),
            );
            return;
        }
        self.board.set_position(game.position());
        self.board.set_last_move(Some((from, to)));
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
                // Why the attempt was wrong, in the engine's words. The tab
                // used to answer a wrong move with nothing at all.
                Reply::Evaluation { analysis, token } => {
                    self.lesson_busy.set(false);
                    if token != self.lesson_token.get() {
                        continue;
                    }
                    let asked = self.asked_about.borrow();
                    let Some((after, played)) = asked.as_ref() else {
                        continue;
                    };
                    match omachess_core::teach::refutation(after, &analysis) {
                        Some(found) => self.lesson.set_label(&found.describe(played)),
                        // Nothing usable back means nothing said, rather than
                        // something confident about a score never given.
                        None => self.lesson.set_label(""),
                    }
                }
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
        drop(slot);

        self.update_countdown();
        self.judge();
    }
}
