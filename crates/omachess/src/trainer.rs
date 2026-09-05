//! The puzzle-solving view: board, clock, and the record of each attempt.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Instant;

use chrono::{Duration as Span, Utc};
use gtk4::prelude::*;
use gtk4::{glib, Align, AspectFrame, Box as GtkBox, Button, Label, Orientation, Switch};
use omachess_core::grade::band;

use omachess_core::puzzle::{Attempt, MoveOutcome, Puzzle};
use omachess_core::session::{Session, Solve};
use omachess_core::store::Store;
use shakmaty::san::San;
use shakmaty::{Move, Position, Square};

use crate::board::BoardView;
use crate::pieces::PieceSet;
use crate::progress_view::ProgressData;
use crate::sound::{Cue, Sounds};

/// How long a rejected move stays highlighted.
const WRONG_FLASH_MS: u32 = 600;
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
    sounds: Rc<Sounds>,
    mode_switch: Switch,
    own_switch: Switch,
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
    current: RefCell<Option<Current>>,
}

impl Trainer {
    pub fn new(
        store: Rc<RefCell<Store>>,
        pieces: Option<Rc<PieceSet>>,
        sounds: Rc<Sounds>,
    ) -> Rc<Self> {
        let board = BoardView::new(pieces);

        let status = Label::builder()
            .label("Loading…")
            .wrap(true)
            .max_width_chars(40)
            .halign(Align::Start)
            .build();
        status.add_css_class("omachess-status");

        let timer = Label::builder().label("0:00").halign(Align::End).build();
        timer.add_css_class("omachess-timer");

        let detail = Label::builder()
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(48)
            .build();
        detail.add_css_class("dim-label");

        // The two modes are a deliberate, visible choice rather than something
        // the app decides quietly: one builds a repertoire, the other measures.
        let mode_switch = Switch::builder().valign(Align::Center).build();
        let mode_label = Label::builder().label("Repeat & measure").build();

        // The positions you actually lost from are the material a coach would
        // pick, and there are only ever a few dozen, so they need asking for.
        let own_switch = Switch::builder().valign(Align::Center).build();
        let own_label = Label::builder().label("Drill my mistakes").build();

        // Filled in when a drill position is finished: which game it came from
        // and what was played instead. Blank for an ordinary puzzle.
        let origin = Label::builder()
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(46)
            .build();
        origin.add_css_class("dim-label");
        let mode_caption = Label::builder()
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(40)
            .build();
        mode_caption.add_css_class("dim-label");

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
        mode_row.append(&own_label);
        mode_row.append(&own_switch);
        mode_row.append(&mode_label);
        mode_row.append(&mode_switch);
        let spacer = GtkBox::builder().hexpand(true).build();
        mode_row.append(&spacer);
        mode_row.append(&timer);

        let header = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(2)
            .build();
        header.append(&mode_row);
        header.append(&status);
        header.append(&mode_caption);
        header.append(&origin);

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

        root.append(&header);
        root.append(&framed);
        root.append(&detail);

        let trainer = Rc::new(Self {
            store,
            session: Session::new(),
            board,
            sounds,
            mode_switch,
            own_switch,
            origin,
            mode_caption,
            start,
            session_started: Cell::new(false),
            root,
            status,
            timer,
            detail,
            current: RefCell::new(None),
        });

        let weak: Weak<Self> = Rc::downgrade(&trainer);
        trainer.start.connect_clicked(move |_| {
            if let Some(trainer) = weak.upgrade() {
                trainer.begin_solving();
            }
        });

        trainer.mode_switch.set_active(trainer.repeat_mode());
        trainer.own_switch.set_active(trainer.own_mistakes_mode());
        trainer.describe_mode();

        let weak: Weak<Self> = Rc::downgrade(&trainer);
        trainer.mode_switch.connect_state_set(move |_, on| {
            if let Some(trainer) = weak.upgrade() {
                trainer.set_repeat_mode(on);
                trainer.describe_mode();
            }
            glib::Propagation::Proceed
        });

        let weak: Weak<Self> = Rc::downgrade(&trainer);
        trainer.own_switch.connect_state_set(move |_, on| {
            if let Some(trainer) = weak.upgrade() {
                trainer.set_own_mistakes_mode(on);
                trainer.describe_mode();
            }
            glib::Propagation::Proceed
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
        let (weaknesses, baseline_success) = p::recurring_weaknesses(&store).unwrap_or_default();
        ProgressData {
            weaknesses,
            baseline_success,
            transfer: p::transfer_by_band(&store).unwrap_or_default(),
            overall: p::measured_improvement(&store).unwrap_or_default(),
            bands: p::improvement_by_band(&store).unwrap_or_default(),
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
            repeat_mode: store.repeat_mode().unwrap_or(false),
        }
    }

    /// Whether the trainer is currently re-testing rather than teaching.
    pub fn repeat_mode(&self) -> bool {
        self.store.borrow().repeat_mode().unwrap_or(false)
    }

    pub fn own_mistakes_mode(&self) -> bool {
        self.store.borrow().own_mistakes_mode().unwrap_or(false)
    }

    /// Draw only from positions taken out of the player's own games.
    pub fn set_own_mistakes_mode(&self, on: bool) {
        let _ = self.store.borrow().set_own_mistakes_mode(on);
    }

    /// Switch between learning new material and re-testing solved material.
    pub fn set_repeat_mode(&self, on: bool) {
        let _ = self.store.borrow().set_repeat_mode(on);
        self.load_next();
    }

    /// Say plainly which mode is on and what it does, so the distinction is
    /// never a hidden detail.
    fn describe_mode(&self) {
        let solved = self.solved_count();
        if self.own_mistakes_mode() && !self.repeat_mode() {
            let (unseen, total) = self
                .store
                .borrow()
                .theme_stock(omachess_core::review::OWN_GAME_THEME)
                .unwrap_or((0, 0));
            self.mode_caption.set_label(&if total == 0 {
                "No positions from your own games yet — import a PGN export first.".to_owned()
            } else if unseen == 0 {
                format!(
                    "All {total} positions from your own games are solved. \
                     Turn on Repeat & measure to re-test them."
                )
            } else {
                format!("{unseen} of {total} positions from your own losses still to face.")
            });
            return;
        }
        self.mode_caption.set_label(&if self.repeat_mode() {
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
                    self.board.set_wrong(false);
                    self.board.set_last_move(None);
                    self.board.set_check(None);
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
                self.status
                    .set_label("Nothing to solve. Load the puzzle database, or come back when reviews are due.");
                *self.current.borrow_mut() = None;
            }
            Err(e) => {
                omachess_core::diagnostics::record_error("trainer::load_next", &e);
                self.status.set_label(&format!("Database error: {e}"));
            }
        }
    }

    /// Begin the session. Only the first puzzle needs this.
    fn begin_solving(&self) {
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

        match self.find_move(from, square) {
            Some(mv) => {
                self.board.select(None);
                self.offer(&mv);
            }
            None => self.board.select(None),
        }
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
        if let Some(mv) = self.find_move(from, to) {
            self.offer(&mv);
        }
    }

    /// True only once the solver has pressed Start on the current puzzle.
    fn solving(&self) -> bool {
        self.current
            .borrow()
            .as_ref()
            .is_some_and(|current| current.started_solving)
    }

    fn in_check(&self) -> bool {
        self.current
            .borrow()
            .as_ref()
            .is_some_and(|c| c.attempt.position().is_check())
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
                self.sounds.play(Cue::Wrong);
                self.reject()
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
                self.sounds.play(cue_for(&reply, self.in_check()));
                self.status.set_label("Good — keep going");
            }
            Ok(MoveOutcome::Solved) => {
                if let Some(current) = self.current.borrow().as_ref() {
                    self.board.set_position(current.attempt.position());
                    self.board
                        .set_check(check_square(current.attempt.position()));
                }
                self.board
                    .set_last_move(mv.from().map(|from| (from, mv.to())));
                self.sounds.play(Cue::Solved);
                self.finish();
            }
            Err(e) => self.status.set_label(&format!("{e}")),
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
        self.board.set_wrong(true);

        // The attempt is already recorded as failed, so withholding the answer
        // past this point teaches nothing.
        if misses >= REVEAL_AFTER_MISSES {
            match self.expected_san() {
                Some(san) => self
                    .status
                    .set_label(&format!("The move is {san} \u{2014} play it to continue")),
                None => self.status.set_label("Not the move"),
            }
        } else {
            self.status.set_label("Not the move \u{2014} try again");
        }

        let board = self.board.clone();
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(u64::from(WRONG_FLASH_MS)),
            move || board.set_wrong(false),
        );
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

/// Which sound a move deserves: check is more worth hearing than a capture,
/// and a capture more than a quiet move.
fn cue_for(mv: &Move, gives_check: bool) -> Cue {
    if gives_check {
        Cue::Check
    } else if mv.is_capture() {
        Cue::Capture
    } else {
        Cue::Move
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
    #[test]
    fn the_repeat_threshold_reads_in_hours() {
        use chrono::Duration;
        use omachess_core::store::MIN_REPEAT_HOURS;
        let threshold = Duration::minutes((MIN_REPEAT_HOURS * 60.0) as i64);
        assert_eq!(humanise(threshold), "20 h");
    }

    /// Check is worth hearing over a capture, and a capture over a quiet move,
    /// so a checking capture must not be announced as an ordinary one.
    #[test]
    fn check_is_heard_over_a_capture() {
        use shakmaty::{Move, Role, Square};
        let capture = Move::Normal {
            role: Role::Queen,
            from: Square::D1,
            capture: Some(Role::Pawn),
            to: Square::D7,
            promotion: None,
        };
        let quiet = Move::Normal {
            role: Role::Queen,
            from: Square::D1,
            capture: None,
            to: Square::D4,
            promotion: None,
        };
        assert_eq!(cue_for(&capture, true), Cue::Check);
        assert_eq!(cue_for(&capture, false), Cue::Capture);
        assert_eq!(cue_for(&quiet, true), Cue::Check);
        assert_eq!(cue_for(&quiet, false), Cue::Move);
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
