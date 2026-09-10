//! The puzzle-solving view: board, clock, and the record of each attempt.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Instant;

use chrono::{Duration as Span, Utc};
use gtk4::prelude::*;
use gtk4::{glib, Align, AspectFrame, Box as GtkBox, Button, Label, Orientation};
use omachess_core::grade::band;

use omachess_core::puzzle::{Attempt, MoveOutcome, Puzzle};
use omachess_core::session::{Session, Solve};
use omachess_core::store::Store;
use omachess_core::vision::{Answer, Rung, NEEDED};
use shakmaty::san::San;
use shakmaty::{Move, Position, Square};

use crate::board::BoardView;
use crate::engine_worker::{EngineWorker, Reply, Request};
use crate::pieces::PieceSet;
use crate::progress_view::ProgressData;

/// How deep the refutation is searched. Deeper than a move in a game needs to
/// be, because this one is being explained rather than played, and the solver
/// is no longer on a clock that counts.
const LESSON_DEPTH: u32 = 18;

/// The three ways the tab works, in the order the picker lists them.
const MODES: [&str; 3] = ["Learn", "Repeat & measure", "Board vision"];

fn mode_key(index: u32) -> &'static str {
    match index {
        1 => "repeat",
        2 => "vision",
        _ => "learn",
    }
}

fn rung_number(rung: Rung) -> usize {
    Rung::LADDER.iter().position(|r| *r == rung).unwrap_or(0) + 1
}

fn mode_index(key: &str) -> u32 {
    match key {
        "repeat" => 1,
        "vision" => 2,
        _ => 0,
    }
}

/// Which answer button was pressed. The pair means light/dark on one rung and
/// yes/no on another, so they are named by side rather than by meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VisionSide {
    A,
    B,
}

/// What the answer was, for the line under the board.
fn describe_answer(drill: &omachess_core::vision::Drill) -> String {
    match &drill.answer {
        Answer::Light => "It is light.".to_owned(),
        Answer::Dark => "It is dark.".to_owned(),
        Answer::Yes => "Yes — it attacks it.".to_owned(),
        Answer::No => "No — it does not.".to_owned(),
        Answer::Squares(set) => {
            let mut named: Vec<String> = set.iter().map(|s| s.to_string()).collect();
            named.sort();
            named.join(" ")
        }
    }
}

fn expected_squares(drill: &omachess_core::vision::Drill) -> Vec<Square> {
    match &drill.answer {
        Answer::Squares(set) => set.iter().copied().collect(),
        _ => Vec::new(),
    }
}

/// A board-vision question in progress.
struct VisionDrill {
    drill: omachess_core::vision::Drill,
    asked: Instant,
    /// Squares clicked so far, for the rungs answered by clicking.
    chosen: Vec<Square>,
}

/// How long a rejected move stays highlighted.
/// Wrong moves before the answer is shown. The attempt already counts as failed
/// by then, so there is nothing left to protect.
const REVEAL_AFTER_MISSES: u32 = 2;

struct Current {
    puzzle: Puzzle,
    attempt: Attempt,
    started: Instant,
    /// When this move began, so each move is timed rather than only the solve.
    move_started: Instant,
    /// Which move of the solution is being answered.
    ply: u32,
    /// When the attempt began, in wall time, so its moves can be joined to it.
    began_at: chrono::DateTime<Utc>,
    /// Set once a wrong move has been played; the attempt still finishes, but
    /// it is recorded as a failure.
    failed: bool,
    /// The clock does not run, and the position is not shown, until the solver
    /// says they are ready.
    started_solving: bool,
    /// Wrong moves so far, used to decide when to reveal the answer.
    misses: u32,
}

pub struct Trainer {
    store: Rc<RefCell<Store>>,
    session: Session,
    board: Rc<BoardView>,
    mode_pick: gtk4::DropDown,
    vision_a: Button,
    vision_b: Button,
    vision_done: Button,
    /// The drill on screen, when the tab is doing board vision.
    vision: RefCell<Option<VisionDrill>>,
    /// What the solver actually played here, once it can be shown.
    origin: Label,
    mode_caption: Label,
    start: Button,
    /// Whether the solver has begun this session. Only the first puzzle waits
    /// for Start; after that the position and the clock appear together, which
    /// is just as honest and does not interrupt a run of solving.
    session_started: Cell<bool>,
    root: GtkBox,
    status: Label,
    timer: Label,
    detail: Label,
    /// Why the last wrong move was wrong, once the engine has said.
    lesson: Label,
    /// The engine, when there is one. Absent is a working trainer without
    /// explanations, never a broken one.
    worker: Option<EngineWorker>,
    /// Identifies the position a refutation was asked about, so an answer that
    /// arrives after the solver has moved on is dropped rather than shown
    /// against a position it was never about.
    lesson_token: Cell<u64>,
    /// Only one question is ever outstanding. Asking again while the engine is
    /// still thinking builds a backlog of answers nobody will read, which is
    /// how the Study tab came to look like it had stopped.
    lesson_busy: Cell<bool>,
    /// How many refutations have been asked for. Read by the self-test: the
    /// engine must never be asked while an attempt can still be measured.
    lesson_asks: Cell<u64>,
    /// The position and the move the outstanding question is about, so the
    /// answer is read against the board it was asked for.
    asked_about: RefCell<Option<(shakmaty::Chess, String)>>,
    current: RefCell<Option<Current>>,
}

impl Trainer {
    pub fn new(
        store: Rc<RefCell<Store>>,
        pieces: Option<Rc<PieceSet>>,
        engine: Option<std::path::PathBuf>,
    ) -> Rc<Self> {
        let board = BoardView::new(pieces);

        let status = Label::builder()
            .label("Loading…")
            .wrap(true)
            .max_width_chars(26)
            .halign(Align::Start)
            .build();
        status.add_css_class("omachess-status");

        let timer = Label::builder().label("0:00").halign(Align::End).build();
        timer.add_css_class("omachess-timer");

        let detail = Label::builder()
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(34)
            .build();
        detail.add_css_class("dim-label");

        // The two modes are a deliberate, visible choice rather than something
        // the app decides quietly: one builds a repertoire, the other measures.
        // Three ways to train, not two. A switch cannot say three things.
        let mode_pick = gtk4::DropDown::from_strings(&MODES);
        mode_pick.set_valign(Align::Center);

        // Board vision: the answer buttons for the rungs that are a yes/no or
        // a light/dark, and the Next that follows a graded answer.
        let vision_a = Button::with_label("Light");
        let vision_b = Button::with_label("Dark");
        let vision_done = Button::with_label("Check");
        vision_done.add_css_class("suggested-action");
        let vision_row = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(6)
            .build();
        vision_row.append(&vision_a);
        vision_row.append(&vision_b);
        vision_row.append(&vision_done);
        vision_row.set_visible(false);

        // Filled in when a drill position is finished: which game it came from
        // and what was played instead. Blank for an ordinary puzzle.
        let origin = Label::builder()
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(34)
            .build();
        origin.add_css_class("dim-label");
        let mode_caption = Label::builder()
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(34)
            .build();
        mode_caption.add_css_class("dim-label");

        // What punishes the move that was just played. Empty until there is
        // something true to put in it.
        let lesson = Label::builder()
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(34)
            .build();
        lesson.add_css_class("omachess-lesson");

        // Timing cannot begin before the solver is looking, so the position is
        // withheld until this is pressed. Showing it first would let anyone
        // solve at leisure and then start the clock.
        let start = Button::with_label("Start");
        start.add_css_class("suggested-action");

        let mode_row = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(8)
            .build();
        mode_row.append(&start);
        mode_row.append(&mode_pick);
        let spacer = GtkBox::builder().hexpand(true).build();
        mode_row.append(&spacer);
        mode_row.append(&timer);

        let header = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(2)
            .build();
        header.append(&mode_row);
        header.append(&status);
        header.append(&vision_row);
        header.append(&mode_caption);
        header.append(&origin);
        header.append(&lesson);

        let root = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        // The board must stay square however the window is resized.
        let framed = AspectFrame::builder()
            .ratio(1.0)
            .obey_child(false)
            .hexpand(true)
            .vexpand(true)
            .child(board.widget())
            .build();

        // Board on the left, everything about it on the right — the same shape
        // as every other tab, and it stops a wide window putting the board in
        // the middle with blank space on both sides.
        let panel = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(8)
            .width_request(300)
            .build();
        panel.append(&header);
        panel.append(&detail);

        // Scrolled so the panel can be narrower than the text wants to be:
        // without it a half-width window pushes the labels off the edge.
        let panel_scroll = gtk4::ScrolledWindow::builder()
            .child(&panel)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .propagate_natural_width(false)
            .hexpand(true)
            .build();

        let columns = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(16)
            .build();
        columns.append(&framed);
        columns.append(&panel_scroll);
        root.append(&columns);

        let trainer = Rc::new(Self {
            store,
            session: Session::new(),
            board,
            mode_pick,
            vision_a,
            vision_b,
            vision_done,
            vision: RefCell::new(None),
            origin,
            mode_caption,
            start,
            session_started: Cell::new(false),
            root,
            status,
            timer,
            detail,
            lesson,
            worker: engine.map(EngineWorker::spawn),
            lesson_token: Cell::new(0),
            lesson_busy: Cell::new(false),
            lesson_asks: Cell::new(0),
            asked_about: RefCell::new(None),
            current: RefCell::new(None),
        });

        let weak: Weak<Self> = Rc::downgrade(&trainer);
        trainer.start.connect_clicked(move |_| {
            if let Some(trainer) = weak.upgrade() {
                trainer.begin_solving();
            }
        });

        // Engine answers arrive on a channel, so something has to look. Weak,
        // so the timer cannot be the reason the trainer stays alive.
        if trainer.worker.is_some() {
            let weak: Weak<Self> = Rc::downgrade(&trainer);
            glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
                match weak.upgrade() {
                    Some(trainer) => {
                        trainer.drain_engine();
                        glib::ControlFlow::Continue
                    }
                    None => glib::ControlFlow::Break,
                }
            });
        }

        trainer
            .mode_pick
            .set_selected(mode_index(&trainer.store.borrow().train_mode().unwrap_or_default()));
        trainer.describe_mode();

        let weak: Weak<Self> = Rc::downgrade(&trainer);
        trainer.mode_pick.connect_selected_notify(move |pick| {
            if let Some(trainer) = weak.upgrade() {
                trainer.set_mode(mode_key(pick.selected()));
            }
        });

        for (button, answer) in [
            (&trainer.vision_a, VisionSide::A),
            (&trainer.vision_b, VisionSide::B),
        ] {
            let weak: Weak<Self> = Rc::downgrade(&trainer);
            button.connect_clicked(move |_| {
                if let Some(trainer) = weak.upgrade() {
                    trainer.answer_vision_button(answer);
                }
            });
        }
        let weak: Weak<Self> = Rc::downgrade(&trainer);
        trainer.vision_done.connect_clicked(move |_| {
            if let Some(trainer) = weak.upgrade() {
                trainer.commit_vision();
            }
        });

        let weak: Weak<Self> = Rc::downgrade(&trainer);
        trainer.board.connect_move(move |square| {
            if let Some(trainer) = weak.upgrade() {
                trainer.on_square(square);
            }
        });

        let weak: Weak<Self> = Rc::downgrade(&trainer);
        trainer.board.connect_drag(move |from, to| {
            if let Some(trainer) = weak.upgrade() {
                trainer.on_drag(from, to);
            }
        });

        let weak: Weak<Self> = Rc::downgrade(&trainer);
        glib::timeout_add_seconds_local(1, move || match weak.upgrade() {
            Some(trainer) => {
                trainer.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });

        trainer.load_next();
        trainer
    }

    /// The board itself, so a test can click it the way a person does.
    pub(crate) fn board(&self) -> &Rc<BoardView> {
        &self.board
    }

    pub fn widget(&self) -> &GtkBox {
        &self.root
    }

    /// A one-line summary of where things stand, for the window subtitle.
    pub fn summary(&self) -> String {
        let store = self.store.borrow();
        let rating = store.personal_rating().unwrap_or_default().round();
        let due = store.due_count(Utc::now()).unwrap_or(0);
        let total = store.count_puzzles().unwrap_or(0);
        if total == 0 {
            return "No puzzles loaded — run `omachess ingest <lichess_db_puzzle.csv.zst>`".into();
        }
        format!(
            "Rating {rating:.0} · {due} due · {} puzzles",
            compact(total)
        )
    }

    /// Everything the progress view draws, gathered in one pass so the figures
    /// and the charts can never disagree about what the data says.
    pub fn progress_data(&self) -> ProgressData {
        use omachess_core::progress as p;
        let store = self.store.borrow();
        let (_, baseline_success) = p::recurring_weaknesses(&store).unwrap_or_default();
        ProgressData {
            baseline_success,
            themes: store
                .theme_success(p::MIN_THEME_ATTEMPTS)
                .unwrap_or_default(),
            transfer: p::transfer_by_band(&store).unwrap_or_default(),
            overall: p::measured_improvement(&store).unwrap_or_default(),
            solved: store.solved_count().unwrap_or(0),
            slopes: p::slope_points(&store).unwrap_or_default(),
            ratings: p::rating_history(&store)
                .unwrap_or_default()
                .into_iter()
                .map(|(_, r)| r)
                .collect(),
            games: p::game_points(&store).unwrap_or_default(),
            play: p::play_trend(&store).unwrap_or_default(),
            endgames: p::endgame_records(&store).unwrap_or_default(),
            openings: p::opening_records(&store).unwrap_or_default(),
            pressure: p::pressure_record(&store).unwrap_or_default(),
        }
    }

    /// Whether the trainer is currently re-testing rather than teaching.
    /// How many refutations have been asked for. For the self-test, which has
    /// to be able to prove the engine stayed out of the measured phase.
    pub(crate) fn lesson_asks(&self) -> u64 {
        self.lesson_asks.get()
    }

    /// Test hooks for the vision ladder.
    pub(crate) fn vision_prompt(&self) -> String {
        self.vision
            .borrow()
            .as_ref()
            .map(|v| v.drill.prompt.clone())
            .unwrap_or_default()
    }

    /// The squares that would answer the drill on screen correctly.
    pub(crate) fn vision_answer(&self) -> Vec<Square> {
        self.vision
            .borrow()
            .as_ref()
            .map(|v| expected_squares(&v.drill))
            .unwrap_or_default()
    }

    pub(crate) fn vision_side_for(&self, want_correct: bool) -> Option<VisionSide> {
        let held = self.vision.borrow();
        let drill = &held.as_ref()?.drill;
        let a_is_right = matches!(drill.answer, Answer::Light | Answer::Yes);
        Some(if a_is_right == want_correct {
            VisionSide::A
        } else {
            VisionSide::B
        })
    }

    pub(crate) fn press_vision(self: &Rc<Self>, side: VisionSide) {
        self.answer_vision_button(side);
    }

    pub(crate) fn press_vision_check(self: &Rc<Self>) {
        self.commit_vision();
    }

    pub(crate) fn choose_mode(&self, mode: &str) {
        self.mode_pick.set_selected(mode_index(mode));
        self.set_mode(mode);
    }

    pub(crate) fn status_text(&self) -> String {
        self.status.text().to_string()
    }

    pub(crate) fn lesson_text(&self) -> String {
        self.lesson.text().to_string()
    }

    pub fn repeat_mode(&self) -> bool {
        self.store.borrow().repeat_mode().unwrap_or(false)
    }

    /// Draw only from positions taken out of the player's own games.
    /// Switch between learning new material and re-testing solved material.
     /// Say plainly which mode is on and what it does, so the distinction is
    /// never a hidden detail.
    /// Switch what the tab is doing.
    fn set_mode(&self, mode: &str) {
        if let Err(e) = self.store.borrow().set_train_mode(mode) {
            omachess_core::diagnostics::record_error("trainer::set_mode", e);
        }
        self.describe_mode();
        if mode == "vision" {
            self.next_vision();
        } else {
            *self.vision.borrow_mut() = None;
            self.vision_row().set_visible(false);
            self.board.set_marks(&[]);
            self.load_next();
        }
    }

    fn vision_row(&self) -> gtk4::Widget {
        self.vision_a
            .parent()
            .expect("the answer buttons live in a row")
    }

    fn rung(&self) -> Rung {
        self.store
            .borrow()
            .vision_rung()
            .ok()
            .and_then(|key| Rung::from_key(&key))
            .unwrap_or(Rung::SquareColour)
    }

    /// Deal the next board-vision question.
    fn next_vision(&self) {
        let rung = self.rung();
        let seed = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64;
        let drill = omachess_core::vision::next(rung, seed);

        self.board.set_orientation(shakmaty::Color::White);
        self.board.set_board(&drill.board);
        self.board.set_marks(&[]);
        self.board.select(None);
        self.board.set_last_move(None);
        self.board.set_check(None);
        self.board.set_mate(None);

        self.status.set_label(&drill.prompt);
        self.lesson.set_label("");
        self.timer.set_label("");
        self.start.set_visible(false);

        // Two buttons for a two-way answer, the board for everything else.
        let two_way = matches!(
            drill.answer,
            Answer::Light | Answer::Dark | Answer::Yes | Answer::No
        );
        let (a, b) = match drill.rung {
            Rung::SquareColour => ("Light", "Dark"),
            _ => ("Yes", "No"),
        };
        self.vision_a.set_label(a);
        self.vision_b.set_label(b);
        self.vision_a.set_visible(two_way);
        self.vision_b.set_visible(two_way);
        self.vision_done.set_visible(!two_way);
        self.vision_row().set_visible(true);

        *self.vision.borrow_mut() = Some(VisionDrill {
            drill,
            asked: Instant::now(),
            chosen: Vec::new(),
        });
        self.describe_mode();
    }

    fn answer_vision_button(self: &Rc<Self>, side: VisionSide) {
        let given = {
            let held = self.vision.borrow();
            let Some(current) = held.as_ref() else {
                return;
            };
            match (current.drill.rung, side) {
                (Rung::SquareColour, VisionSide::A) => Answer::Light,
                (Rung::SquareColour, VisionSide::B) => Answer::Dark,
                (_, VisionSide::A) => Answer::Yes,
                (_, VisionSide::B) => Answer::No,
            }
        };
        self.grade_vision(&given);
    }

    /// The clicked squares, offered as the answer.
    fn commit_vision(self: &Rc<Self>) {
        let given = {
            let held = self.vision.borrow();
            let Some(current) = held.as_ref() else {
                return;
            };
            Answer::Squares(current.chosen.iter().copied().collect())
        };
        self.grade_vision(&given);
    }

    /// A click during board vision picks a square rather than moving a piece.
    fn choose_square(&self, square: Square) {
        let marks = {
            let mut held = self.vision.borrow_mut();
            let Some(current) = held.as_mut() else {
                return;
            };
            if let Some(at) = current.chosen.iter().position(|s| *s == square) {
                current.chosen.remove(at);
            } else {
                current.chosen.push(square);
            }
            current.chosen.clone()
        };
        self.board.set_marks(&marks);
    }

    fn grade_vision(self: &Rc<Self>, given: &Answer) {
        let Some(current) = self.vision.borrow_mut().take() else {
            return;
        };
        let took = current.asked.elapsed();
        let right = omachess_core::vision::judge(&current.drill, given);

        // Recorded on its own, never in `attempts`: that table is one row per
        // puzzle and feeds every speed figure in the application. A drill
        // answered in two seconds is not a puzzle solved in two seconds.
        let rung = current.drill.rung;
        {
            let store = self.store.borrow();
            if let Err(e) = store.record_vision(rung.key(), chrono::Utc::now(), right, took) {
                omachess_core::diagnostics::record_error("trainer::record_vision", e);
            }
        }

        if right {
            self.status.set_label(&format!("Right — {:.1}s", took.as_secs_f64()));
        } else {
            self.status.set_label("Not that.");
            self.board.set_marks(&expected_squares(&current.drill));
        }
        self.lesson.set_label(&describe_answer(&current.drill));

        // Climb when the rung has been climbed, rather than after a count.
        let recent = self
            .store
            .borrow()
            .vision_recent(rung.key(), NEEDED as u32)
            .unwrap_or_default();
        if omachess_core::vision::ready_to_promote(&recent, rung) {
            if let Some(up) = rung.next() {
                if let Err(e) = self.store.borrow().set_vision_rung(up.key()) {
                    omachess_core::diagnostics::record_error("trainer::promote", e);
                }
                self.lesson
                    .set_label(&format!("{} — next: {}", describe_answer(&current.drill), up.label()));
            }
        }

        let weak: Weak<Self> = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_millis(if right { 500 } else { 2200 }), move || {
            if let Some(trainer) = weak.upgrade() {
                if trainer.store.borrow().train_mode().unwrap_or_default() == "vision" {
                    trainer.next_vision();
                }
            }
        });
    }

    fn describe_mode(&self) {
        let mode = self.store.borrow().train_mode().unwrap_or_default();
        if mode == "vision" {
            let rung = self.rung();
            let (asked, right) = self
                .store
                .borrow()
                .vision_record(rung.key())
                .unwrap_or((0, 0));
            self.mode_caption.set_label(&if asked == 0 {
                format!("{} — {} of 6.", rung.label(), rung_number(rung))
            } else {
                format!(
                    "{} — {right} of {asked} right.",
                    rung.label()
                )
            });
            return;
        }
        let solved = self.solved_count();
        self.mode_caption.set_label(&if mode == "repeat" {
            format!(
                "Re-testing {solved} solved puzzle{} — these attempts are measured.",
                if solved == 1 { "" } else { "s" }
            )
        } else {
            "New puzzles, rising in difficulty. Building repertoire; not measured.".to_owned()
        });
    }

    /// How many distinct puzzles have been solved at least once.
    pub fn solved_count(&self) -> u64 {
        self.store.borrow().solved_count().unwrap_or(0)
    }

    fn load_next(&self) {
        // A new position is walked into blind, so whatever the last one
        // revealed is cleared before it appears.
        self.origin.set_label("");
        self.origin.remove_css_class("warning");
        // A refutation belongs to the position it was asked about. Bumping the
        // token retires any answer still in flight for the puzzle just left.
        self.lesson.set_label("");
        self.lesson_token.set(self.lesson_token.get().wrapping_add(1));
        *self.asked_about.borrow_mut() = None;
        crate::announce::clear();
        let next = {
            let store = self.store.borrow();
            self.session.next_puzzle(&store, Utc::now())
        };

        match next {
            Ok(Some(puzzle)) => match Attempt::new(&puzzle) {
                Ok(attempt) => {
                    // The solver plays the side to move, so show it from their side.
                    self.board.set_orientation(attempt.position().turn());
                    self.board.clear();
                    self.board.select(None);
                    self.board.set_last_move(None);
                    self.board.set_check(None);
                    self.board.set_mate(None);
                    let side = match attempt.position().turn() {
                        shakmaty::Color::White => "White",
                        shakmaty::Color::Black => "Black",
                    };
                    self.detail.set_label(&format!(
                        "Rating {} · {}",
                        puzzle.rating,
                        puzzle.themes.join(", ")
                    ));

                    // The puzzle must be in place before anything reveals it;
                    // revealing first reads an empty slot and shows nothing.
                    *self.current.borrow_mut() = Some(Current {
                        puzzle,
                        attempt,
                        started: Instant::now(),
                        move_started: Instant::now(),
                        ply: 0,
                        began_at: Utc::now(),
                        failed: false,
                        started_solving: false,
                        misses: 0,
                    });
                    self.timer.set_label("0:00");

                    if self.session_started.get() {
                        // Mid-session: reveal and time in the same instant, so
                        // nothing can be studied while the clock is stopped.
                        self.reveal(side);
                    } else {
                        self.status
                            .set_label(&format!("{side} to play — press Start when ready"));
                        self.start.set_sensitive(true);
                        self.start.set_visible(true);
                    }
                }
                Err(e) => self.status.set_label(&format!("Skipping bad puzzle: {e}")),
            },
            Ok(None) => {
                // In Repeat mode "nothing to solve" is both wrong and useless:
                // there is plenty solved, it is simply too soon for any of it to
                // measure anything. Saying when to come back is the difference
                // between a broken tab and a schedule.
                let waiting = if self.repeat_mode() {
                    self.store
                        .borrow()
                        .hours_until_repeat(Utc::now())
                        .unwrap_or(None)
                } else {
                    None
                };
                self.status.set_label(&match waiting {
                    Some(hours) => format!(
                        "Nothing far enough back to re-time yet — the first is ready in {}. \
                         Turn Repeat off to keep learning new puzzles meanwhile.",
                        describe_wait(hours)
                    ),
                    None => "Nothing to solve. Load the puzzle database, or come back when \
                             reviews are due."
                        .to_owned(),
                });
                *self.current.borrow_mut() = None;
            }
            Err(e) => {
                omachess_core::diagnostics::record_error("trainer::load_next", &e);
                self.status.set_label(&format!("Database error: {e}"));
            }
        }
    }

    /// Begin the session. Only the first puzzle needs this.
    pub(crate) fn begin_solving(&self) {
        // A sitting is opened once, on the first puzzle, so every attempt can
        // be filed by how deep into the session it was. Whether the twentieth
        // solve goes worse than the second is a training question, and it
        // cannot be asked at all without this.
        if let Err(e) = self
            .session
            .open_sitting(&self.store.borrow(), "puzzles", Utc::now())
        {
            omachess_core::diagnostics::record_error("trainer::open_sitting", e);
        }

        self.session_started.set(true);
        let side = match self
            .current
            .borrow()
            .as_ref()
            .map(|c| c.attempt.position().turn())
        {
            Some(shakmaty::Color::White) => "White",
            Some(shakmaty::Color::Black) => "Black",
            None => return,
        };
        self.reveal(side);
    }

    /// Show the position and start timing it, in the same instant.
    fn reveal(&self, side: &str) {
        let Some(position) = self.current.borrow_mut().as_mut().map(|current| {
            current.started_solving = true;
            current.started = Instant::now();
            current.attempt.position().clone()
        }) else {
            return;
        };
        self.board.set_position(&position);
        self.board.set_check(check_square(&position));
        self.start.set_visible(false);
        self.status.set_label(&format!("{side} to play"));
    }

    fn tick(&self) {
        if let Some(current) = self.current.borrow().as_ref() {
            if !current.started_solving {
                self.timer.set_label("0:00");
                return;
            }
            let secs = current.started.elapsed().as_secs();
            self.timer
                .set_label(&format!("{}:{:02}", secs / 60, secs % 60));
        }
    }

    fn on_square(self: &Rc<Self>, square: Square) {
        // In board vision a click chooses a square rather than moving a piece:
        // most of the rungs are arrangements no move is legal in.
        if self.vision.borrow().is_some() {
            self.choose_square(square);
            return;
        }
        if !self.solving() {
            return;
        }

        let Some(from) = self.board.selected() else {
            if self.has_own_piece(square) {
                self.board.select(Some(square));
            }
            return;
        };

        if from == square {
            self.board.select(None);
            return;
        }

        // Re-selecting one's own piece changes the origin rather than failing.
        if self.has_own_piece(square) && self.find_move(from, square).is_none() {
            self.board.select(Some(square));
            return;
        }

        self.board.select(None);
        self.offer_move(from, square);
    }

    /// The expected move in algebraic notation, for revealing the answer.
    fn expected_san(&self) -> Option<String> {
        let current = self.current.borrow();
        let current = current.as_ref()?;
        let position = current.attempt.position();
        let expected = current.attempt.expected()?;
        let mv = current.attempt.parse_move(expected).ok()?;
        Some(San::from_move(position, mv).to_string())
    }

    /// Say so when this sitting has stopped helping.
    ///
    /// The useful moment to learn that a session has gone downhill is during
    /// it, not in a report a week later. Solving badly and tired teaches the
    /// habit of solving badly.
    fn check_fatigue(&self) {
        let Some(sitting) = self.session.sitting() else {
            return;
        };
        let reading = omachess_core::progress::sitting_fatigue(&self.store.borrow(), sitting);
        let Ok(Some((early, late, count))) = reading else {
            return;
        };
        if early - late < omachess_core::progress::FATIGUE_DROP {
            return;
        }
        // The drill's own explanation was just written here and is worth
        // keeping; this is added to it rather than over it.
        let warning = format!(
            "{count} in, and this sitting has turned: {:.0}% right early against {:.0}% now. \
             Solving tired trains solving badly — a good place to stop.",
            early * 100.0,
            late * 100.0,
        );
        let existing = self.origin.label();
        self.origin.set_label(&if existing.is_empty() {
            warning
        } else {
            format!("{existing}\n\n{warning}")
        });
        self.origin.add_css_class("warning");
    }

    /// Say what was played in the game this position came from.
    ///
    /// Shown only once the position is answered: beforehand it would give the
    /// move away, and the point of a drill is to walk in blind.
    fn show_origin(&self) {
        let id = {
            let current = self.current.borrow();
            match current.as_ref() {
                Some(current) => current.puzzle.id.clone(),
                None => return,
            }
        };
        let origin = self.store.borrow().drill_origin(&id).ok().flatten();
        let Some(origin) = origin else {
            self.origin.set_label("");
            return;
        };
        let moves = origin.ply / 2 + 1;
        self.origin.set_label(&format!(
            "From your game on {}, move {moves}: you played {} and it cost {:.0}% \
             of the win. The move was {}.",
            origin.played_at.format("%-d %b"),
            origin.played,
            origin.lost * 100.0,
            origin.best,
        ));
    }

    /// The move the puzzle is asking for, in the notation the raw log keeps.
    fn expected_uci(&self) -> Option<String> {
        let current = self.current.borrow();
        current.as_ref()?.attempt.expected().map(str::to_owned)
    }

    /// A piece dragged straight from one square to another.
    fn on_drag(self: &Rc<Self>, from: Square, to: Square) {
        if !self.solving() || !self.has_own_piece(from) {
            return;
        }
        self.board.select(None);
        self.offer_move(from, to);
    }

    /// True only once the solver has pressed Start on the current puzzle.
    pub(crate) fn solving(&self) -> bool {
        self.current
            .borrow()
            .as_ref()
            .is_some_and(|current| current.started_solving)
    }

     fn has_own_piece(&self, square: Square) -> bool {
        self.current
            .borrow()
            .as_ref()
            .map(|c| {
                let position = c.attempt.position();
                position.board().piece_at(square).map(|p| p.color) == Some(position.turn())
            })
            .unwrap_or(false)
    }

    /// Delegates to the core, which knows that castling is stored as
    /// king-takes-rook and that a promotion shares its squares with three
    /// other moves.
    fn find_move(&self, from: Square, to: Square) -> Option<Move> {
        let current = self.current.borrow();
        let current = current.as_ref()?;
        omachess_core::game::find_move(
            current.attempt.position(),
            from,
            to,
            current.attempt.expected(),
        )
    }

    /// Offer a move, asking which piece first when a pawn reaches the last
    /// rank. A puzzle whose answer is an underpromotion cannot be solved by a
    /// board that always queens.
    pub(crate) fn offer_move(self: &Rc<Self>, from: Square, to: Square) {
        let choices = {
            let current = self.current.borrow();
            match current.as_ref() {
                Some(current) => {
                    omachess_core::game::promotion_choices(current.attempt.position(), from, to)
                }
                None => Vec::new(),
            }
        };
        if choices.len() > 1 {
            let white = self
                .current
                .borrow()
                .as_ref()
                .map(|current| current.attempt.position().turn() == shakmaty::Color::White)
                .unwrap_or(true);
            let trainer = self.clone();
            self.board.ask_promotion(to, white, &choices, move |role| {
                let prefer = crate::play_view::promotion_uci(from, to, role);
                let found = {
                    let current = trainer.current.borrow();
                    current.as_ref().and_then(|current| {
                        omachess_core::game::find_move(
                            current.attempt.position(),
                            from,
                            to,
                            Some(&prefer),
                        )
                    })
                };
                if let Some(mv) = found {
                    trainer.offer(&mv);
                }
            });
            return;
        }
        if let Some(mv) = self.find_move(from, to) {
            self.offer(&mv);
        }
    }

    fn offer(self: &Rc<Self>, mv: &Move) {
        // Captured before the move is judged: afterwards the attempt has moved
        // on and the answer that was expected here is no longer reachable.
        let context = {
            let current = self.current.borrow();
            current.as_ref().map(|current| {
                (
                    current.puzzle.id.clone(),
                    current.began_at,
                    current.ply,
                    current.move_started.elapsed(),
                    current.misses >= REVEAL_AFTER_MISSES,
                    self.expected_uci().unwrap_or_default(),
                )
            })
        };
        let outcome = {
            let mut current = self.current.borrow_mut();
            let Some(current) = current.as_mut() else {
                return;
            };
            current.attempt.play(mv)
        };

        // Every move offered is written down, right or wrong. The wrong ones
        // are the point: a solver who reaches for the same losing idea across
        // twenty positions has one habit, and the verdict alone never shows it.
        if let Some((puzzle_id, began_at, ply, thought, revealed, expected)) = context {
            let correct = matches!(
                outcome,
                Ok(MoveOutcome::Continued(_)) | Ok(MoveOutcome::Solved)
            );
            let played = mv.to_uci(shakmaty::CastlingMode::Standard).to_string();
            let recorded = self.store.borrow().record_attempt_move(
                self.session.sitting(),
                &puzzle_id,
                began_at,
                ply,
                &played,
                &expected,
                correct,
                thought,
                revealed,
            );
            if let Err(e) = recorded {
                omachess_core::diagnostics::record_error("trainer::record_attempt_move", e);
            }
            if let Some(current) = self.current.borrow_mut().as_mut() {
                current.move_started = Instant::now();
                if correct {
                    current.ply += 1;
                }
            }
        }

        match outcome {
            Ok(MoveOutcome::Wrong) => {
                self.reject();
                // Only now. A wrong move has just set `failed`, and every
                // measurement in this application counts correct attempts
                // alone — so from here the clock on this puzzle is already out
                // of the figures and the engine cannot distort one. Asking a
                // move earlier would put engine latency inside the very number
                // the application exists to report.
                self.ask_why(mv);
            }
            Ok(MoveOutcome::Continued(reply)) => {
                if let Some(current) = self.current.borrow().as_ref() {
                    self.board.set_position(current.attempt.position());
                    self.board
                        .set_check(check_square(current.attempt.position()));
                }
                // Highlight the opponent's reply rather than our own move: that
                // is the change the solver has to read before answering.
                self.board
                    .set_last_move(reply.from().map(|from| (from, reply.to())));
                self.status.set_label("Good — keep going");
            }
            Ok(MoveOutcome::Solved) => {
                if let Some(current) = self.current.borrow().as_ref() {
                    self.board.set_position(current.attempt.position());
                    self.board
                        .set_check(check_square(current.attempt.position()));
                    // Most puzzles end in mate, and the board has a louder mark
                    // for it than for check. Only Play was ever using it, so
                    // every mating puzzle finished looking like an ordinary
                    // check — the one thing the board most needs to say, said
                    // in the quieter of its two voices.
                    self.board
                        .set_mate(mate_square(current.attempt.position()));
                }
                self.board
                    .set_last_move(mv.from().map(|from| (from, mv.to())));
                self.finish();
            }
            Err(e) => self.status.set_label(&format!("{e}")),
        }
    }

    /// Ask what punishes the move that was just played.
    ///
    /// The question is about the position the mistake leads to, which nobody
    /// has analysed: the puzzle's author never considered it. That is exactly
    /// what an engine is for, and it is the only thing in this loop one is
    /// asked.
    fn ask_why(&self, mv: &Move) {
        self.lesson.set_label("");
        let Some(worker) = &self.worker else {
            return;
        };
        // One question at a time. A solver trying four wrong moves in a row
        // would otherwise queue four searches and read the answer to the first.
        if self.lesson_busy.get() {
            return;
        }
        let after = {
            let current = self.current.borrow();
            let Some(current) = current.as_ref() else {
                return;
            };
            let mut position = current.attempt.position().clone();
            position.play_unchecked(*mv);
            position
        };
        let played = {
            let current = self.current.borrow();
            match current.as_ref() {
                Some(current) => shakmaty::san::SanPlus::from_move(
                    current.attempt.position().clone(),
                    *mv,
                )
                .to_string(),
                None => return,
            }
        };

        let token = self.lesson_token.get().wrapping_add(1);
        self.lesson_token.set(token);
        *self.asked_about.borrow_mut() = Some((after.clone(), played));

        let fen =
            shakmaty::fen::Fen::from_position(&after, shakmaty::EnPassantMode::Legal).to_string();
        if worker.send(Request::Evaluate {
            fen,
            moves: Vec::new(),
            depth: LESSON_DEPTH,
            token,
        }) {
            self.lesson_busy.set(true);
            self.lesson_asks.set(self.lesson_asks.get() + 1);
            self.lesson.set_label("Looking at why…");
        }
    }

    /// Take whatever the engine has said, and say it in one line.
    fn drain_engine(&self) {
        let Some(worker) = &self.worker else {
            return;
        };
        // Bounded: an unbounded drain on the thread drawing the window is one
        // bad reply away from freezing it.
        let mut budget = 16;
        while let Some(reply) = worker.poll() {
            budget -= 1;
            if budget < 0 {
                break;
            }
            match reply {
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
                        // An engine that returned nothing usable says nothing.
                        // A blank line is honest; a confident one would not be.
                        None => self.lesson.set_label(""),
                    }
                }
                Reply::Failed(_) => {
                    self.lesson_busy.set(false);
                    self.lesson.set_label("");
                }
                _ => {}
            }
        }
        if worker.is_finished() {
            self.lesson_busy.set(false);
        }
    }

    fn reject(&self) {
        let misses = {
            let mut current = self.current.borrow_mut();
            match current.as_mut() {
                Some(current) => {
                    current.failed = true;
                    current.misses += 1;
                    current.misses
                }
                None => return,
            }
        };
        // The attempt is already recorded as failed, so withholding the answer
        // past this point teaches nothing.
        let said = if misses >= REVEAL_AFTER_MISSES {
            match self.expected_san() {
                Some(san) => format!("Not the move. It is {san} \u{2014} play it to continue"),
                None => "Not the move".to_owned(),
            }
        } else {
            "Not the move \u{2014} try again".to_owned()
        };
        // Said once. The status line sits beside the board and carries this,
        // and the line under it explains why; announcing the same sentence
        // across the window as well put it on screen twice, the second time in
        // thirty-point type.
        self.status.set_label(&said);
    }

    fn finish(self: &Rc<Self>) {
        self.show_origin();
        self.check_fatigue();

        let Some(current) = self.current.borrow_mut().take() else {
            return;
        };

        let solve = Solve {
            puzzle_id: current.puzzle.id.clone(),
            puzzle_rating: current.puzzle.rating,
            correct: !current.failed,
            elapsed: Span::from_std(current.started.elapsed()).unwrap_or_else(|_| Span::zero()),
        };

        let result = {
            let mut store = self.store.borrow_mut();
            self.session.submit(&mut store, &solve, Utc::now())
        };

        match result {
            Ok(outcome) => {
                let seconds = solve.elapsed.num_milliseconds() as f64 / 1000.0;
                let baseline = outcome.baseline.num_milliseconds() as f64 / 1000.0;
                let verdict = if solve.correct {
                    format!("Solved in {seconds:.1}s (your pace for this level: {baseline:.1}s)")
                } else {
                    format!("Missed it — back soon. {seconds:.1}s")
                };
                self.status.set_label(&verdict);
                self.detail.set_label(&format!(
                    "{:?} · next in {} · rating {:.0} · band {}",
                    outcome.grade,
                    humanise(outcome.due - Utc::now()),
                    outcome.personal_rating,
                    band(current.puzzle.rating),
                ));
            }
            Err(e) => self
                .status
                .set_label(&format!("Could not record attempt: {e}")),
        }

        let weak: Weak<Self> = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_millis(1400), move || {
            if let Some(trainer) = weak.upgrade() {
                trainer.load_next();
            }
        });
    }
}

/// Millions of puzzles do not fit in a window subtitle, and the exact figure
/// is not what the reader wants from it.
fn compact(count: u64) -> String {
    match count {
        0..=9_999 => count.to_string(),
        10_000..=999_999 => format!("{:.0}k", count as f64 / 1_000.0),
        _ => format!("{:.1}M", count as f64 / 1_000_000.0),
    }
}

fn humanise(span: Span) -> String {
    let minutes = span.num_minutes();
    if minutes < 1 {
        "under a minute".into()
    } else if minutes < 60 {
        format!("{minutes} min")
    } else if span.num_hours() < 48 {
        format!("{} h", span.num_hours())
    } else {
        format!("{} days", span.num_days())
    }
}

/// The square of the king that is currently in check, if any.
/// The mated king, if the position on the board is checkmate.
fn mate_square(position: &impl Position) -> Option<Square> {
    position
        .is_checkmate()
        .then(|| position.board().king_of(position.turn()))
        .flatten()
}

/// A wait in the units a person would use for it.
fn describe_wait(hours: f64) -> String {
    let hours = hours.max(0.0);
    if hours < 1.0 {
        let minutes = (hours * 60.0).ceil().max(1.0) as i64;
        return format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" });
    }
    let whole = hours.round() as i64;
    format!("{whole} hour{}", if whole == 1 { "" } else { "s" })
}

fn check_square(position: &impl Position) -> Option<Square> {
    position
        .is_check()
        .then(|| position.board().king_of(position.turn()))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_shortened_only_once_they_are_long() {
        assert_eq!(compact(0), "0");
        assert_eq!(compact(9_999), "9999");
        assert_eq!(compact(97_167), "97k");
        assert_eq!(compact(999_999), "1000k");
        assert_eq!(compact(4_342_467), "4.3M");
    }

    /// A span is read to judge whether a repeat is far enough from the first
    /// solve to mean anything, so the units have to change where a reader
    /// would change them.
    #[test]
    fn spans_are_read_the_way_a_person_would_say_them() {
        use chrono::Duration;
        assert_eq!(humanise(Duration::seconds(30)), "under a minute");
        assert_eq!(humanise(Duration::minutes(1)), "1 min");
        assert_eq!(humanise(Duration::minutes(59)), "59 min");
        assert_eq!(humanise(Duration::minutes(60)), "1 h");
        assert_eq!(humanise(Duration::hours(47)), "47 h");
        // Two days is where hours stop being readable.
        assert_eq!(humanise(Duration::hours(48)), "2 days");
        assert_eq!(humanise(Duration::days(9)), "9 days");
    }

    /// The twenty-hour rule is the project's central guard, so the boundary it
    /// is described at must not drift into reading as minutes.
    /// The wait is the whole message, so it has to read like something a
    /// person would say rather than a float.
    #[test]
    fn a_wait_is_stated_the_way_it_would_be_spoken() {
        assert_eq!(describe_wait(0.0), "1 minute");
        assert_eq!(describe_wait(0.25), "15 minutes");
        assert_eq!(describe_wait(1.0), "1 hour");
        assert_eq!(describe_wait(19.4), "19 hours");
        // A negative wait means it is already ready, and must never be spoken
        // as one.
        assert_eq!(describe_wait(-3.0), "1 minute");
    }

    #[test]
    fn the_repeat_threshold_reads_in_hours() {
        use chrono::Duration;
        use omachess_core::store::MIN_REPEAT_HOURS;
        let threshold = Duration::minutes((MIN_REPEAT_HOURS * 60.0) as i64);
        assert_eq!(humanise(threshold), "20 h");
    }

    /// The king in check is highlighted, and it must be the king of the side to
    /// move — highlighting the other one would point at the wrong danger.
    #[test]
    fn the_highlighted_king_is_the_one_actually_in_check() {
        use shakmaty::fen::Fen;
        use shakmaty::{CastlingMode, Chess, Square};
        let position = |fen: &str| {
            fen.parse::<Fen>()
                .unwrap()
                .into_position::<Chess>(CastlingMode::Standard)
                .unwrap()
        };
        // Black is in check from the queen on e7; Black is to move.
        let checked = position("4k3/4Q3/8/8/8/8/8/4K3 b - - 0 1");
        assert_eq!(check_square(&checked), Some(Square::E8));

        let quiet = position("4k3/8/8/8/8/8/8/4K3 w - - 0 1");
        assert_eq!(check_square(&quiet), None);
    }
}
