//! Playing a game against the engine.
//!
//! The point of playing here is not the game — it is what the game leaves
//! behind. Every position the player misjudged becomes a puzzle in the same
//! deck as everything else, so a loss on Tuesday is a drill on Friday.
//!
//! There is deliberately no evaluation bar and no takeback. Both would tell the
//! player they had gone wrong before they had a chance to notice, which is the
//! one skill a game trains that a puzzle cannot.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk4::prelude::*;
use gtk4::{
    glib, Align, AspectFrame, Box as GtkBox, Button, DropDown, Label, ListBox, Orientation,
    PolicyType, ScrolledWindow, SelectionMode,
};
use omachess_core::drill::{Drill, Offer};
use omachess_core::engine::MIN_LIMITED_ELO;
use omachess_core::game::{material_balance, whose_turn, Turn};
use omachess_core::game::{EndReason, Game, Verdict};
use omachess_core::openings;
use omachess_core::puzzle::Puzzle;
use omachess_core::review::{
    describe_move, puzzle_from, stable_puzzle_id, time_pressure, GameAnalysis, MoveAnalysis,
};
use omachess_core::store::Store;
use shakmaty::{Color, Move, Position, Square};
use std::time::Instant;

use crate::board::BoardView;
use crate::engine_worker::{EngineWorker, Reply, Request};
use crate::pieces::PieceSet;
use crate::sound::{Cue, Sounds};

/// How often replies from the engine thread are collected.
const POLL_MS: u32 = 80;

pub struct PlayView {
    root: GtkBox,
    board: Rc<BoardView>,
    sounds: Rc<Sounds>,
    store: Rc<RefCell<Store>>,
    worker: Option<EngineWorker>,
    game: RefCell<Option<Game>>,
    status: Label,
    detail: Label,
    moves: Label,
    side: DropDown,
    start: Button,
    resign: Button,
    /// Material difference from the player's point of view.
    material: Label,
    /// The opening being played, which is knowledge rather than a hint.
    opening: Label,
    /// Flagged moves after the game, one row each.
    review_list: ListBox,
    review_scroll: ScrolledWindow,
    /// Think time for each of the player's own moves.
    move_times: RefCell<Vec<std::time::Duration>>,
    turn_started: Cell<Option<Instant>>,
    /// The clock, absent when the game is untimed.
    clock: RefCell<Option<omachess_core::clock::Clock>>,
    /// Which time control the picker is on.
    control: DropDown,
    opening_pick: DropDown,
    /// Openings worth drilling, in the order the picker lists them after the
    /// "From the start" entry.
    drillable_openings: RefCell<Vec<(String, Vec<String>)>>,
    /// The opening this game was set up in, and how many plies of it were
    /// played before the player took over. Empty for a game from move one.
    drilling: RefCell<Option<(String, u32)>>,
    /// The sitting these games belong to, opened on the first one.
    sitting: Cell<Option<i64>>,
    clock_mine: Label,
    /// For each of the player's own moves, whether it was made on a low clock.
    /// Kept beside the move times so a blunder can be attributed to the clock.
    pressured: RefCell<Vec<bool>>,
    /// The finished game's analysis, awaiting the player's decision.
    report: RefCell<Option<GameAnalysis>>,
    /// Result headline, shown only once the game is actually over.
    banner: Label,
    /// The position where the game turned, offered back as practice.
    drill: RefCell<Option<Drill>>,
    drill_button: Button,
    add_button: Button,
    plan_box: GtkBox,
    thinking: Cell<bool>,
}

impl PlayView {
    pub fn new(
        store: Rc<RefCell<Store>>,
        pieces: Option<Rc<PieceSet>>,
        sounds: Rc<Sounds>,
        engine: Option<std::path::PathBuf>,
    ) -> Rc<Self> {
        let board = BoardView::new(pieces);
        let worker = engine.map(EngineWorker::spawn);

        let status = Label::builder()
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(34)
            .build();
        status.add_css_class("omachess-status");

        let detail = Label::builder()
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(34)
            .build();
        detail.add_css_class("dim-label");

        let moves = Label::builder()
            .halign(Align::Start)
            .valign(Align::Start)
            .wrap(true)
            .selectable(true)
            .build();
        moves.add_css_class("monospace");

        let side = DropDown::from_strings(&["Play White", "Play Black"]);

        // An untimed game trains a skill nobody uses in a rated game, so a time
        // control is offered first and "Untimed" has to be chosen deliberately.
        let mut control_labels: Vec<&str> = omachess_core::clock::PRESETS
            .iter()
            .map(|(name, _)| *name)
            .collect();
        control_labels.push("Untimed");
        let control = DropDown::from_strings(&control_labels);

        // Only the player is on a clock. The engine moves in a fixed few
        // hundred milliseconds and is not the one being trained, so a second
        // clock would be decoration standing in for a real opponent.
        // Filled in when the view is shown: which openings are worth drilling
        // depends on games that may not have been played yet.
        let opening_pick = DropDown::from_strings(&["From the start"]);

        let clock_mine = Label::builder().halign(Align::Start).build();
        clock_mine.add_css_class("omachess-clock");
        let start = Button::with_label("New game");
        start.add_css_class("suggested-action");

        let add_button = Button::with_label("Add mistakes to training");
        add_button.set_visible(false);

        let drill_button = Button::with_label("Practise the moment it turned");
        drill_button.add_css_class("suggested-action");
        drill_button.set_visible(false);

        let resign = Button::with_label("Resign");
        resign.set_visible(false);

        // A finished game must never look like one still in progress.
        let banner = Label::builder()
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(30)
            .visible(false)
            .build();
        banner.add_css_class("omachess-banner");

        let material = Label::builder().halign(Align::Start).build();
        material.add_css_class("dim-label");

        // Naming the opening teaches something without revealing anything about
        // the position in front of the player, so it is safe to show in play.
        let opening = Label::builder()
            .halign(Align::Start)
            .wrap(true)
            .max_width_chars(34)
            .build();
        opening.add_css_class("dim-label");

        let controls = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(8)
            .build();
        controls.append(&side);
        controls.append(&control);
        controls.append(&opening_pick);
        controls.append(&start);
        controls.append(&resign);

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

        let review_list = ListBox::builder()
            .selection_mode(SelectionMode::None)
            .build();
        review_list.add_css_class("boxed-list");
        let review_scroll = ScrolledWindow::builder()
            .child(&review_list)
            .hscrollbar_policy(PolicyType::Never)
            .vexpand(true)
            .build();
        review_scroll.set_visible(false);

        // Before a game there is nothing to say about the position, and the
        // panel was simply empty. What to train is the useful thing to put in
        // front of someone who has just opened the application.
        let plan_box = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(4)
            .build();

        let moves_heading = Label::builder().label("Moves").halign(Align::Start).build();
        moves_heading.add_css_class("heading");

        // The panel beside the board is where the study happens: what you are
        // playing, how long you spend, and — once the game is over — every
        // position you misjudged. Nothing here reveals an evaluation while the
        // game is still running.
        // The board is limited by the window height, so on a wide screen the
        // space left over is the panel's. It expands to take it rather than
        // reserving a column and leaving the rest blank.
        let panel = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(8)
            .width_request(340)
            .hexpand(true)
            .build();
        panel.append(&controls);
        panel.append(&clock_mine);
        panel.append(&banner);
        panel.append(&status);
        panel.append(&detail);
        panel.append(&material);
        panel.append(&opening);
        panel.append(&moves_heading);
        panel.append(&move_scroll);
        panel.append(&review_scroll);
        panel.append(&drill_button);
        panel.append(&add_button);
        panel.append(&plan_box);

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
            .spacing(10)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        root.append(&content);

        let view = Rc::new(Self {
            root,
            board,
            sounds,
            store,
            worker,
            game: RefCell::new(None),
            status,
            detail,
            moves,
            side,
            start,
            resign,
            material,
            opening,
            review_list,
            review_scroll,
            move_times: RefCell::new(Vec::new()),
            turn_started: Cell::new(None),
            clock: RefCell::new(None),
            control,
            opening_pick,
            drillable_openings: RefCell::new(Vec::new()),
            drilling: RefCell::new(None),
            sitting: Cell::new(None),
            clock_mine,
            pressured: RefCell::new(Vec::new()),
            report: RefCell::new(None),
            banner,
            drill: RefCell::new(None),
            drill_button,
            add_button,
            plan_box,
            thinking: Cell::new(false),
        });

        let weak: Weak<Self> = Rc::downgrade(&view);
        view.board.connect_move(move |square| {
            if let Some(view) = weak.upgrade() {
                view.on_square(square);
            }
        });

        let weak: Weak<Self> = Rc::downgrade(&view);
        view.board.connect_drag(move |from, to| {
            if let Some(view) = weak.upgrade() {
                view.on_drag(from, to);
            }
        });

        let weak: Weak<Self> = Rc::downgrade(&view);
        view.start.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.start_game();
            }
        });

        let weak: Weak<Self> = Rc::downgrade(&view);
        view.resign.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.resign_game();
            }
        });

        let weak: Weak<Self> = Rc::downgrade(&view);
        view.drill_button.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.start_drill();
            }
        });

        let weak: Weak<Self> = Rc::downgrade(&view);
        view.add_button.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.add_found_to_training();
            }
        });

        let weak: Weak<Self> = Rc::downgrade(&view);
        glib::timeout_add_local(
            std::time::Duration::from_millis(u64::from(POLL_MS)),
            move || match weak.upgrade() {
                Some(view) => {
                    view.drain_engine();
                    view.tick_clock();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            },
        );

        view.show_idle_state();

        view
    }

    pub fn widget(&self) -> &GtkBox {
        &self.root
    }

    fn show_idle_state(&self) {
        self.show_plan();
        // An empty board reads as a broken application, not as a waiting one.
        // Nothing ever put a position here before a game began.
        if self.game.borrow().is_none() {
            let side = if self.side.selected() == 0 {
                Color::White
            } else {
                Color::Black
            };
            self.board.set_orientation(side);
            self.board.set_position(&shakmaty::Chess::default());
        }
        match &self.worker {
            Some(worker) => {
                self.status.set_label("Ready when you are.");
                self.detail.set_label(&format!(
                    "Opponent: {} capped near {} Elo. No evaluation is shown while you play; \
                     your mistakes are collected and offered as puzzles afterwards.",
                    worker.name(),
                    self.opponent_elo()
                ));
                self.start.set_sensitive(true);
            }
            None => {
                self.status.set_label("No engine found.");
                self.detail.set_label(
                    "Install one and restart:  yay -S stockfish\n\
                     OMACHESS looks for `stockfish` on your PATH.",
                );
                self.start.set_sensitive(false);
            }
        }
    }

    /// The strength to cap the opponent at: a little above the player, and
    /// never below what the engine is willing to do.
    /// Refresh the list of openings worth drilling from the games on record.
    ///
    /// Called when the view is shown rather than once at construction: the
    /// openings that cost the player games change as more are played.
    pub fn refresh_openings(&self) {
        let found =
            omachess_core::progress::openings_to_drill(&self.store.borrow()).unwrap_or_default();
        let mut labels = vec!["From the start".to_owned()];
        let mut lines = Vec::new();
        for (record, moves) in found {
            labels.push(format!(
                "{} ({:.0}%, {} games)",
                record.name,
                record.score() * 100.0,
                record.games
            ));
            lines.push((record.name, moves));
        }
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        self.opening_pick
            .set_model(Some(&gtk4::StringList::new(&refs)));
        self.opening_pick.set_selected(0);
        *self.drillable_openings.borrow_mut() = lines;
    }

    /// The opening line the picker is on, if it is not "From the start".
    fn chosen_opening(&self) -> Option<(String, Vec<String>)> {
        let index = self.opening_pick.selected() as usize;
        index
            .checked_sub(1)
            .and_then(|i| self.drillable_openings.borrow().get(i).cloned())
    }

    /// The time control the picker is on, or `None` for an untimed game.
    fn chosen_control(&self) -> Option<omachess_core::clock::TimeControl> {
        control_at(self.control.selected() as usize)
    }

    /// Paint the clock, including the time being spent on the move in progress.
    fn show_clocks(&self) {
        let guard = self.clock.borrow();
        let Some(clock) = guard.as_ref() else {
            self.clock_mine.set_label("");
            self.clock_mine.set_visible(false);
            return;
        };
        self.clock_mine.set_visible(true);

        let mine = self
            .game
            .borrow()
            .as_ref()
            .map(|game| game.player())
            .unwrap_or(Color::White);
        // Only count the current think while it is actually the player's turn.
        let thinking = match self.turn_started.get() {
            Some(started) => started.elapsed(),
            None => std::time::Duration::ZERO,
        };
        let left = clock.showing(mine, thinking);
        self.clock_mine
            .set_label(&format!("Your clock  {}", format_clock(left)));

        let low = left < clock.control().pressure_threshold();
        if low {
            self.clock_mine.add_css_class("error");
        } else {
            self.clock_mine.remove_css_class("error");
        }
    }

    /// Keep the clock moving, and end the game the moment it runs out rather
    /// than waiting for a move that is never going to come.
    fn tick_clock(&self) {
        let Some(started) = self.turn_started.get() else {
            return;
        };
        let mine = {
            let guard = self.game.borrow();
            match guard.as_ref() {
                Some(game) if game.outcome().is_none() => game.player(),
                _ => return,
            }
        };
        let expired = match self.clock.borrow_mut().as_mut() {
            Some(clock) => clock.expire(mine, started.elapsed()),
            None => return,
        };
        if expired {
            self.flag(mine);
        } else {
            self.show_clocks();
        }
    }

    /// End the game because a clock ran out.
    fn flag(&self, ran_out: Color) {
        use omachess_core::clock::{flag_outcome, Flag};
        let outcome = {
            let guard = self.game.borrow();
            let Some(game) = guard.as_ref() else {
                return;
            };
            flag_outcome(game.position(), ran_out)
        };
        // Resigning is the only way the game type can end without a move, and
        // losing on time is the same shape of result.
        if let Some(game) = self.game.borrow_mut().as_mut() {
            game.resign();
        }
        self.turn_started.set(None);
        self.show_clocks();
        self.banner.set_visible(true);
        self.banner.set_label(match outcome {
            Flag::Lost(_) => "Lost on time.",
            Flag::DrawnByInsufficientMaterial(_) => {
                "Out of time — drawn, the engine cannot mate with what it has left."
            }
        });
        self.finish_game();
    }

    fn opponent_elo(&self) -> u32 {
        let rating = self
            .store
            .borrow()
            .play_rating()
            .unwrap_or(f64::from(MIN_LIMITED_ELO));
        ((rating + omachess_core::game::OPPONENT_MARGIN)
            .round()
            .max(0.0) as u32)
            .max(MIN_LIMITED_ELO)
    }

    fn start_game(&self) {
        let player = if self.side.selected() == 0 {
            Color::White
        } else {
            Color::Black
        };
        // Starting inside a line the player keeps losing puts the practice
        // where the losses are, instead of replaying known moves first.
        //
        // The game begins at the position the line reaches rather than at move
        // one, so the book moves are never analysed as the player's own
        // decisions — counting them would inflate accuracy and flatten the
        // opening loss, corrupting the very figures that picked the line.
        let opening = self.chosen_opening();
        let game = match &opening {
            None => Game::new(player),
            Some((_, moves)) => {
                let mut position = shakmaty::Chess::default();
                for uci in moves {
                    let Some(mv) = uci
                        .parse::<shakmaty::uci::UciMove>()
                        .ok()
                        .and_then(|parsed| parsed.to_move(&position).ok())
                    else {
                        break;
                    };
                    position = match position.clone().play(mv) {
                        Ok(next) => next,
                        Err(_) => break,
                    };
                }
                let fen =
                    shakmaty::fen::Fen::from_position(&position, shakmaty::EnPassantMode::Legal)
                        .to_string();
                Game::from_fen(player, &fen).unwrap_or_else(|| Game::new(player))
            }
        };
        if self.sitting.get().is_none() {
            match self
                .store
                .borrow()
                .begin_session("play", chrono::Utc::now())
            {
                Ok(id) => self.sitting.set(Some(id)),
                Err(e) => omachess_core::diagnostics::record_error("play::begin_session", e),
            }
        }
        *self.drilling.borrow_mut() = opening
            .as_ref()
            .map(|(name, moves)| (name.clone(), moves.len() as u32));
        self.board.set_orientation(player);
        self.board.set_position(game.position());
        self.board.select(None);
        self.board.set_last_move(None);
        self.board.set_check(None);
        self.moves.set_label("");
        *self.report.borrow_mut() = None;
        self.add_button.set_visible(false);
        self.banner.set_visible(false);
        self.board.set_mate(None);
        *self.drill.borrow_mut() = None;
        self.drill_button.set_visible(false);
        self.opening.set_label("");
        *self.game.borrow_mut() = Some(game);
        self.move_times.borrow_mut().clear();
        self.pressured.borrow_mut().clear();
        *self.clock.borrow_mut() = self.chosen_control().map(omachess_core::clock::Clock::new);
        self.turn_started.set(Some(Instant::now()));
        self.show_clocks();
        self.show_plan();
        self.resign.set_visible(true);
        self.review_scroll.set_visible(false);
        clear_list(&self.review_list);
        self.update_material();
        match &opening {
            Some((name, _)) => self.status.set_label(&format!("{name}. Play on.")),
            None => self.status.set_label("Your move."),
        }
        self.detail.set_label(&format!(
            "Opponent capped near {} Elo.",
            self.opponent_elo()
        ));
        self.request_engine_if_due();
    }

    /// Give up. A resigned game is still reviewed — usually it is the one most
    /// worth reviewing.
    fn resign_game(self: &Rc<Self>) {
        if self.game.borrow().is_none() {
            return;
        }
        if let Some(game) = self.game.borrow_mut().as_mut() {
            game.resign();
        }
        self.finish_game();
    }

    fn on_square(self: &Rc<Self>, square: Square) {
        if self.drilling() {
            let Some(from) = self.board.selected() else {
                self.board.select(Some(square));
                return;
            };
            self.board.select(None);
            if from != square {
                self.drill_move(from, square);
            }
            return;
        }
        if !self.player_may_move() {
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
        if self.has_own_piece(square) && self.find_move(from, square).is_none() {
            self.board.select(Some(square));
            return;
        }
        self.board.select(None);
        self.offer_move(from, square);
    }

    fn on_drag(self: &Rc<Self>, from: Square, to: Square) {
        if self.drilling() {
            self.board.select(None);
            self.drill_move(from, to);
            return;
        }
        if !self.player_may_move() || !self.has_own_piece(from) {
            trace(&format!(
                "no move offered from {from:?}: not your turn or not your piece"
            ));
            return;
        }
        if self.find_move(from, to).is_none() {
            trace(&format!("no legal move from {from:?} to {to:?}"));
        }
        self.board.select(None);
        self.offer_move(from, to);
    }

    /// Which sound a move deserves, judged before the position moves on.
    fn cue_for(&self, mv: &Move) -> Cue {
        let gives_check = self
            .game
            .borrow()
            .as_ref()
            .is_some_and(|game| game.position().is_check());
        if gives_check {
            Cue::Check
        } else if mv.is_capture() {
            Cue::Capture
        } else {
            Cue::Move
        }
    }

    /// True while the post-game exercise is waiting for an answer.
    fn drilling(&self) -> bool {
        self.drill
            .borrow()
            .as_ref()
            .is_some_and(|drill| !drill.is_solved())
    }

    fn player_may_move(&self) -> bool {
        !self.thinking.get()
            && self
                .game
                .borrow()
                .as_ref()
                .is_some_and(|game| whose_turn(game) == Turn::Player)
    }

    fn has_own_piece(&self, square: Square) -> bool {
        self.game
            .borrow()
            .as_ref()
            .map(|game| {
                let position = game.position();
                position.board().piece_at(square).map(|p| p.color) == Some(position.turn())
            })
            .unwrap_or(false)
    }

    /// Delegates to the core, which knows that castling is stored as
    /// king-takes-rook and that a promotion shares its squares with three
    /// other moves.
    fn find_move(&self, from: Square, to: Square) -> Option<Move> {
        let game = self.game.borrow();
        omachess_core::game::find_move(game.as_ref()?.position(), from, to, None)
    }

    /// Put today's session in the panel while there is no game to describe.
    fn show_plan(&self) {
        while let Some(child) = self.plan_box.first_child() {
            self.plan_box.remove(&child);
        }
        if self.game.borrow().is_some() {
            self.plan_box.set_visible(false);
            return;
        }
        let plan = omachess_core::plan::todays_plan(&self.store.borrow()).unwrap_or_default();
        if plan.is_empty() {
            self.plan_box.set_visible(false);
            return;
        }
        self.plan_box.set_visible(true);

        let heading = Label::builder().label("Today").halign(Align::Start).build();
        heading.add_css_class("heading");
        self.plan_box.append(&heading);

        for (index, step) in plan.iter().enumerate() {
            let line = Label::builder()
                .label(format!(
                    "{}. {}  ({} min)",
                    index + 1,
                    step.headline(),
                    step.minutes
                ))
                .halign(Align::Start)
                .wrap(true)
                .build();
            self.plan_box.append(&line);
            // The reason is the part worth reading; without it this is a list
            // of chores rather than a session.
            let why = Label::builder()
                .label(&step.why)
                .halign(Align::Start)
                .wrap(true)
                .max_width_chars(56)
                .build();
            why.add_css_class("dim-label");
            self.plan_box.append(&why);
        }
    }

    /// Play a move, asking which piece first when a pawn reaches the last rank.
    ///
    /// Every route onto the board goes through here, so the question is asked
    /// once however the piece was moved.
    fn offer_move(self: &Rc<Self>, from: Square, to: Square) {
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
                let prefer = promotion_uci(from, to, role);
                let found = {
                    let game = view.game.borrow();
                    game.as_ref().and_then(|game| {
                        omachess_core::game::find_move(game.position(), from, to, Some(&prefer))
                    })
                };
                if let Some(mv) = found {
                    view.play_player_move(&mv);
                }
            });
            return;
        }
        if let Some(mv) = self.find_move(from, to) {
            self.play_player_move(&mv);
        }
    }

    fn play_player_move(&self, mv: &Move) {
        {
            let mut game = self.game.borrow_mut();
            let Some(game) = game.as_mut() else {
                return;
            };
            if game.play(mv).is_err() {
                return;
            }
        }
        if let Some(started) = self.turn_started.take() {
            let thinking = started.elapsed();
            self.move_times.borrow_mut().push(thinking);
            // Recorded before the clock is charged: what matters is how much
            // time was showing while the move was being chosen.
            let mover = {
                let guard = self.game.borrow();
                guard.as_ref().map(|game| game.player())
            };
            // Logged before the clock is charged, so the record says what was
            // showing while the move was being chosen rather than after.
            {
                let (subject, ply, uci) = {
                    let guard = self.game.borrow();
                    match guard.as_ref() {
                        Some(game) => (
                            game.initial_fen().to_owned(),
                            game.moves().len().saturating_sub(1) as u32,
                            game.moves().last().cloned().unwrap_or_default(),
                        ),
                        None => (String::new(), 0, String::new()),
                    }
                };
                let detail = match (mover, self.clock.borrow().as_ref()) {
                    (Some(mover), Some(clock)) => format!(
                        "{{\"clock_ms\":{},\"pressured\":{}}}",
                        clock.showing(mover, thinking).as_millis(),
                        clock.under_pressure(mover, thinking)
                    ),
                    _ => "{\"clock_ms\":null,\"pressured\":false}".to_owned(),
                };
                if let Err(e) = self.store.borrow().log_move(
                    self.sitting.get(),
                    "play",
                    &subject,
                    chrono::Utc::now(),
                    ply,
                    &uci,
                    thinking,
                    &detail,
                ) {
                    omachess_core::diagnostics::record_error("play::log_move", e);
                }
            }

            if let (Some(mover), Some(clock)) = (mover, self.clock.borrow_mut().as_mut()) {
                self.pressured
                    .borrow_mut()
                    .push(clock.under_pressure(mover, thinking));
                if !clock.commit(mover, thinking) {
                    self.flag(mover);
                    return;
                }
            }
        }
        self.show_clocks();
        self.after_move(Some((mv.from(), mv.to())));
        self.sounds.play(self.cue_for(mv));
        self.request_engine_if_due();
    }

    fn after_move(&self, last: Option<(Option<Square>, Square)>) {
        let guard = self.game.borrow();
        let Some(game) = guard.as_ref() else {
            return;
        };
        self.board.set_position(game.position());
        self.board
            .set_last_move(last.and_then(|(from, to)| from.map(|from| (from, to))));
        self.board.set_check(
            game.position()
                .is_check()
                .then(|| game.position().board().king_of(game.position().turn()))
                .flatten(),
        );
        self.moves
            .set_label(&format_moves(game, &self.move_times.borrow()));
        drop(guard);
        self.update_material();
        self.update_opening();
    }

    /// Name the line being played, and say when it left the book.
    fn update_opening(&self) {
        let text = match self.game.borrow().as_ref() {
            Some(game) => match openings::identify(game.moves()) {
                Some(found) => {
                    let played = game.moves().len();
                    if played > found.plies {
                        format!(
                            "{} ({})\nout of book since move {}",
                            found.name,
                            found.eco,
                            found.plies / 2 + 1
                        )
                    } else {
                        format!("{} ({})", found.name, found.eco)
                    }
                }
                None => String::new(),
            },
            None => String::new(),
        };
        self.opening.set_label(&text);
    }

    /// Material difference from the player's side, which is factual and already
    /// visible on the board — unlike an evaluation, which is not.
    fn update_material(&self) {
        let text = match self.game.borrow().as_ref() {
            Some(game) => {
                let balance = material_balance(game.position(), game.player());
                match balance.cmp(&0) {
                    std::cmp::Ordering::Greater => format!("Material  +{balance}"),
                    std::cmp::Ordering::Less => format!("Material  {balance}"),
                    std::cmp::Ordering::Equal => "Material  level".to_owned(),
                }
            }
            None => String::new(),
        };
        self.material.set_label(&text);
    }

    fn request_engine_if_due(&self) {
        let (fen, moves, finished) = {
            let game = self.game.borrow();
            let Some(game) = game.as_ref() else {
                return;
            };
            (
                game.initial_fen().to_owned(),
                game.moves().to_vec(),
                game.outcome().is_some(),
            )
        };

        if finished {
            self.finish_game();
            return;
        }
        if self
            .game
            .borrow()
            .as_ref()
            .is_some_and(Game::is_player_turn)
        {
            self.status.set_label("Your move.");
            return;
        }

        let Some(worker) = &self.worker else {
            return;
        };
        self.thinking.set(true);
        self.status.set_label("Thinking…");
        worker.send(Request::Move {
            fen,
            moves,
            elo: self.opponent_elo(),
        });
    }

    fn finish_game(&self) {
        self.thinking.set(false);
        let (verdict, fen, moves, player) = {
            let game = self.game.borrow();
            let Some(game) = game.as_ref() else {
                return;
            };
            (
                game.verdict(),
                game.initial_fen().to_owned(),
                game.moves().to_vec(),
                game.player(),
            )
        };

        self.sounds.play(Cue::End);
        self.resign.set_visible(false);
        self.turn_started.set(None);

        let (reason, mated) = {
            let game = self.game.borrow();
            match game.as_ref() {
                Some(game) => (game.end_reason(), game.mated_king()),
                None => (None, None),
            }
        };
        self.board.set_mate(mated);
        self.board.set_check(None);
        // The reason matters more than the result: "checkmated" tells you
        // something, "you lost" does not.
        let outcome = match verdict {
            Some(Verdict::Won) => "You won",
            Some(Verdict::Lost) => "You lost",
            Some(Verdict::Drawn) => "Draw",
            None => "Game over",
        };
        self.banner.set_label(&match reason {
            Some(reason) => format!("{} — {outcome}", reason.label()),
            None => outcome.to_owned(),
        });
        self.banner.remove_css_class("improving");
        self.banner.remove_css_class("slowing");
        self.banner.add_css_class(match verdict {
            Some(Verdict::Won) => "improving",
            _ => "slowing",
        });
        self.banner.set_visible(true);
        self.status.set_label(match reason {
            Some(EndReason::Checkmate) => "The game is over.",
            Some(EndReason::Resigned) => "You resigned.",
            Some(EndReason::Stalemate | EndReason::InsufficientMaterial) => {
                "Neither side can win from here."
            }
            None => "The game is over.",
        });

        let Some(worker) = &self.worker else {
            return;
        };
        self.detail
            .set_label("Reviewing the game at full strength…");
        worker.send(Request::Review { fen, moves, player });
    }

    /// Collect whatever the engine thread has produced.
    fn drain_engine(self: &Rc<Self>) {
        let Some(worker) = &self.worker else {
            return;
        };
        // Bounded regardless of what the worker does: an unbounded drain on the
        // UI thread is one bad reply away from freezing the window.
        let mut budget = 32;
        while let Some(reply) = worker.poll() {
            budget -= 1;
            if budget < 0 {
                break;
            }
            match reply {
                Reply::Move(uci) => {
                    self.thinking.set(false);
                    let played = {
                        let mut game = self.game.borrow_mut();
                        match game.as_mut() {
                            Some(game) => game
                                .parse_move(&uci)
                                .ok()
                                .filter(|mv| game.play(mv).is_ok())
                                .map(|mv| (mv.from(), mv.to(), mv)),
                            None => None,
                        }
                    };
                    if let Some((_, _, ref mv)) = played {
                        self.after_move(played.map(|(from, to, _)| (from, to)));
                        self.sounds.play(self.cue_for(mv));
                        self.turn_started.set(Some(Instant::now()));
                        self.request_engine_if_due();
                    }
                }
                Reply::Review(analysis) => {
                    self.record_game(&analysis);
                    let times = self.move_times.borrow().clone();
                    self.detail.set_label(&describe(&analysis, &times));
                    let drillable = analysis.drillable().len();
                    let has_moment = analysis.critical_moment().is_some();
                    *self.report.borrow_mut() = Some(analysis);
                    if drillable > 0 {
                        self.populate_review();
                        self.add_button.set_visible(true);
                        self.drill_button.set_visible(has_moment);
                    } else {
                        self.add_button.set_visible(false);
                    }
                }
                // Evaluations belong to the study view; play never asks for one.
                Reply::Evaluation { .. } => {}
                Reply::Failed(message) => {
                    self.thinking.set(false);
                    self.status.set_label(&format!("Engine: {message}"));
                }
            }
        }

        // A worker that has gone cannot come back, so say so plainly and stop
        // offering games rather than leaving the board looking playable.
        if worker.is_finished() && self.start.is_sensitive() {
            self.thinking.set(false);
            self.start.set_sensitive(false);
            self.resign.set_visible(false);
            self.detail.set_label(
                "The engine is no longer running. Restart OMACHESS to play again — \
                 puzzles and progress are unaffected.",
            );
        }
    }

    /// Put the player back in the position where the game turned.
    /// Put the player back in the position where the game turned.
    fn start_drill(self: &Rc<Self>) {
        let Some((drill, ply, lost)) = ({
            let report = self.report.borrow();
            report
                .as_ref()
                .and_then(GameAnalysis::critical_moment)
                .and_then(|m| Drill::from_analysis(m).map(|d| (d, m.ply, m.lost())))
        }) else {
            return;
        };

        self.board.set_orientation(drill.position().turn());
        self.board.set_position(drill.position());
        self.board.set_mate(None);
        self.board.set_last_move(None);
        self.board.select(None);
        self.banner.set_visible(false);

        self.status.set_label("Find the move.");
        self.detail.set_label(&format!(
            "Move {} is where the game turned — you gave away {:.0}% here. Find the move; \
             the answer is played for you and then you find the next one, all the way \
             through the line.",
            ply / 2 + 1,
            lost * 100.0
        ));
        self.drill_button.set_visible(false);
        *self.drill.borrow_mut() = Some(drill);
    }

    /// Judge a move offered during the drill.
    /// Judge a move offered during the exercise.
    /// Judge a move offered during the exercise and walk the line forward.
    fn drill_move(self: &Rc<Self>, from: Square, to: Square) {
        let outcome = match self.drill.borrow_mut().as_mut() {
            Some(drill) => drill.offer(from, to),
            None => return,
        };

        match outcome {
            // A misdrag is not an answer, so it costs nothing.
            Offer::Illegal => {}
            Offer::Correct { reply, finished } => {
                self.sounds.play(Cue::Solved);
                self.show_drill_position(reply.as_deref());
                self.status.set_label(if finished {
                    "That is the whole line."
                } else {
                    "Right. Now find the next one."
                });
                self.describe_drill(finished, None);
            }
            Offer::Wrong { revealed: None, .. } => {
                self.sounds.play(Cue::Wrong);
                self.board.set_wrong(true);
                let board = self.board.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
                    board.set_wrong(false)
                });
                self.status.set_label("Not that one. Look again.");
            }
            Offer::Wrong {
                revealed: Some(answer),
                reply,
                finished,
            } => {
                self.sounds.play(Cue::Move);
                self.show_drill_position(reply.as_deref());
                self.status.set_label(if finished {
                    "That was the line."
                } else {
                    "Shown — keep going from here."
                });
                self.describe_drill(finished, Some(&answer));
            }
        }
    }

    /// Redraw the exercise position after the line has moved on.
    fn show_drill_position(&self, reply: Option<&str>) {
        let Some(position) = self.drill.borrow().as_ref().map(|d| d.position().clone()) else {
            return;
        };
        self.board.set_position(&position);
        self.board.select(None);
        // Highlight the answer that was played, so it is not missed.
        if let Some(reply) = reply.and_then(|uci| uci.parse::<shakmaty::uci::UciMove>().ok()) {
            if let (Some(from), Some(to)) = (reply.from(), reply.to()) {
                self.board.set_last_move(Some((from, to)));
            }
        }
        self.board.set_check(
            position
                .is_check()
                .then(|| position.board().king_of(position.turn()))
                .flatten(),
        );
    }

    /// Say where the solver is in the line, and what was just shown.
    fn describe_drill(&self, finished: bool, revealed: Option<&str>) {
        let progress = self
            .drill
            .borrow()
            .as_ref()
            .map(omachess_core::drill::Drill::progress);
        let mut text = String::new();
        if let Some(answer) = revealed {
            text.push_str(&format!("The move was {answer}. "));
        }
        match (finished, progress) {
            (true, _) => text.push_str(
                "That is what the position was worth. Add it to training to see it again.",
            ),
            (false, Some((done, total))) => {
                text.push_str(&format!("Move {} of {total} in the line.", done + 1))
            }
            (false, None) => {}
        }
        self.detail.set_label(&text);
    }

    /// Play the engine's line so the player sees what the move was for.
    /// Play out the line so the player sees what the move was for.
    /// Store how well the game was played, so quality can be tracked over time
    /// independently of how many were won.
    fn record_game(&self, analysis: &GameAnalysis) {
        if analysis.is_empty() {
            return;
        }
        let Some((player_white, result, opening_name, book_plies)) =
            self.game.borrow().as_ref().map(|game| {
                let (name, plies) = match self.drilling.borrow().clone() {
                    Some(drilled) => drilled,
                    None => match omachess_core::openings::identify(game.moves()) {
                        Some(opening) => (opening.name, opening.plies as u32),
                        None => (String::new(), 0),
                    },
                };
                (
                    game.player() == Color::White,
                    match game.verdict() {
                        Some(Verdict::Won) => "won",
                        Some(Verdict::Lost) => "lost",
                        _ => "drawn",
                    }
                    .to_owned(),
                    name,
                    plies,
                )
            })
        else {
            return;
        };
        let counts = analysis.counts();
        // Which of the player's blunders were played on a low clock. The
        // analysis indexes the player's own moves in the order they were made,
        // which is the same order the pressure flags were recorded in.
        let pressured = self.pressured.borrow();
        let pressure_moves = pressured.iter().filter(|low| **low).count() as u32;
        let pressure_blunders = analysis
            .moves
            .iter()
            .enumerate()
            .filter(|(index, review)| {
                review.severity == Some(omachess_core::review::Severity::Blunder)
                    && pressured.get(*index).copied().unwrap_or(false)
            })
            .count() as u32;
        let time_control = self
            .chosen_control()
            .map(|control| control.label())
            .unwrap_or_default();

        let record = omachess_core::store::GameRecord {
            time_control,
            pressure_moves,
            pressure_blunders,
            opening: opening_name,
            book_plies,
            // Played here, so it is this machine's user regardless of the name
            // a PGN import happens to have remembered.
            player: String::new(),
            played_at: chrono::Utc::now(),
            player_white,
            opponent_elo: self.opponent_elo(),
            result,
            moves: analysis.moves.len() as u32,
            accuracy: analysis.accuracy(),
            mean_loss: analysis.mean_loss(),
            blunders: counts.blunders as u32,
            mistakes: counts.mistakes as u32,
            inaccuracies: counts.inaccuracies as u32,
            source: String::new(),
            phases: phase_losses(analysis),
        };
        let opponent = f64::from(record.opponent_elo);
        {
            let store = self.store.borrow();
            // A game that cannot be filed is a game you played and lost the
            // record of, so the failure is written down rather than shrugged at.
            if let Err(e) = store.record_game(&record) {
                omachess_core::diagnostics::record_error("play::record_game", e);
            }
            // Playing strength follows results against the engine, not puzzles.
            if let Ok(current) = store.play_rating() {
                if let Err(e) = store.set_play_rating(omachess_core::game::next_play_rating(
                    current,
                    opponent,
                    &record.result,
                )) {
                    omachess_core::diagnostics::record_error("play::set_play_rating", e);
                }
            }
        }
    }

    /// List every flagged move, so the player can see what went wrong rather
    /// than being told only how many times it did.
    fn populate_review(self: &Rc<Self>) {
        clear_list(&self.review_list);
        let report = self.report.borrow();
        let Some(analysis) = report.as_ref() else {
            return;
        };
        for review in analysis.drillable() {
            let row = GtkBox::builder()
                .orientation(Orientation::Vertical)
                .spacing(2)
                .margin_top(6)
                .margin_bottom(6)
                .margin_start(8)
                .margin_end(8)
                .build();

            let heading = Label::builder()
                .label(format!(
                    "Move {} — {}",
                    review.ply / 2 + 1,
                    review.severity.map(|s| s.label()).unwrap_or("inaccuracy")
                ))
                .halign(Align::Start)
                .build();
            heading.add_css_class("heading");

            let detail = Label::builder()
                .label(format!(
                    "played {}, better was {} ({:.0}% given away)",
                    review.played,
                    review.best,
                    review.lost() * 100.0
                ))
                .halign(Align::Start)
                .build();
            detail.add_css_class("dim-label");

            row.append(&heading);
            row.append(&detail);

            // Clicking a row puts that position on the board.
            let click = gtk4::GestureClick::new();
            let weak: Weak<Self> = Rc::downgrade(self);
            let fen = review.setup_fen.clone();
            let setup = review.setup_move.clone();
            click.connect_released(move |_, _, _, _| {
                if let Some(view) = weak.upgrade() {
                    view.show_position(&fen, &setup);
                }
            });
            row.add_controller(click);

            self.review_list.append(&row);
        }
        self.review_scroll.set_visible(true);
    }

    /// Put a reviewed position on the board so it can be looked at.
    fn show_position(&self, setup_fen: &str, setup_move: &str) {
        // Reconstructing the position is the core's job; this only draws it.
        let Some(position) = omachess_core::drill::position_after(setup_fen, setup_move) else {
            return;
        };
        self.board.set_position(&position);
        self.status
            .set_label("Reviewing — this is the position you misjudged.");
    }

    /// Turn the collected mistakes into puzzles in the player's own deck.
    fn add_found_to_training(&self) {
        let report = self.report.borrow();
        let Some(analysis) = report.as_ref() else {
            return;
        };
        let reviews: Vec<&MoveAnalysis> = analysis.drillable();
        if reviews.is_empty() {
            return;
        }
        let rating = self
            .store
            .borrow()
            .personal_rating()
            .unwrap_or(f64::from(MIN_LIMITED_ELO))
            .round()
            .max(0.0) as u32;

        let puzzles: Vec<Puzzle> = reviews
            .iter()
            .map(|review| puzzle_from(review, rating, &stable_puzzle_id(review)))
            .collect();

        match self.store.borrow_mut().insert_puzzles(&puzzles) {
            Ok(count) => {
                self.detail
                    .set_label(&format!("Added {count} of your own positions to training."));
                self.add_button.set_visible(false);
            }
            Err(e) => {
                self.detail.set_label(&format!("Could not save: {e}"));
                return;
            }
        }

        // Without this a position from a game played here is a puzzle with no
        // history: the drill cannot say what was played or what it was worth,
        // which is the entire difference between a drill and a puzzle.
        let now = chrono::Utc::now();
        let store = self.store.borrow();
        for review in &reviews {
            if let Err(e) = store.record_drill_origin(
                &stable_puzzle_id(review),
                "",
                now,
                review.ply as u32,
                &describe_move(review, &review.played),
                &describe_move(review, &review.best),
                review.lost(),
                review.phase.theme(),
                review.win_before,
            ) {
                omachess_core::diagnostics::record_error("play::record_drill_origin", e);
            }
        }
    }
}

/// A stable id for a position the player got wrong, so blundering the same way
/// twice updates one puzzle rather than accumulating duplicates.
/// Per-phase loss in the order the store expects: opening, middlegame, endgame.
fn phase_losses(analysis: &GameAnalysis) -> [omachess_core::store::PhaseLoss; 3] {
    use omachess_core::review::Phase;
    use omachess_core::store::PhaseLoss;
    let by_phase = analysis.by_phase();
    [Phase::Opening, Phase::Middlegame, Phase::Endgame].map(|want| {
        by_phase
            .iter()
            .find(|(phase, _, _)| *phase == want)
            .map(|(_, loss, moves)| PhaseLoss {
                mean_loss: *loss,
                moves: *moves as u32,
            })
            .unwrap_or(PhaseLoss::UNKNOWN)
    })
}

/// The quality of a game, stated without reference to who won.
///
/// A win against a weak opponent and a loss against a strong one say little
/// about how well you played. The win probability you gave away per move says
/// a great deal, and it is comparable between games.
fn describe(analysis: &GameAnalysis, times: &[std::time::Duration]) -> String {
    if analysis.is_empty() {
        return "Not enough of the game could be analysed.".to_owned();
    }
    let counts = analysis.counts();
    let mut text = format!(
        "Accuracy {:.0}%  ·  {:.1}% given away per move\n{} blunder{}, {} mistake{}, {} inaccurac{}",
        analysis.accuracy(),
        analysis.mean_loss() * 100.0,
        counts.blunders,
        if counts.blunders == 1 { "" } else { "s" },
        counts.mistakes,
        if counts.mistakes == 1 { "" } else { "s" },
        counts.inaccuracies,
        if counts.inaccuracies == 1 { "y" } else { "ies" },
    );
    // The weakest phase is the single most actionable line in the report.
    if let Some((phase, loss, moves)) = analysis.by_phase().first() {
        text.push_str(&format!(
            "\nWeakest phase: {} ({:.1}% per move over {moves} moves)",
            phase.label(),
            loss * 100.0
        ));
    }

    // Whether the errors came from moving quickly is a different diagnosis from
    // not knowing the position, and calls for a different fix.
    if let Some(pressure) = time_pressure(&analysis.moves, times) {
        let seconds = |d: std::time::Duration| d.as_secs_f64();
        text.push_str(&format!(
            "\n\nQuick moves (under {:.0}s) gave away {:.1}% each; \
             considered moves {:.1}%.",
            seconds(pressure.median_time),
            pressure.quick_loss * 100.0,
            pressure.considered_loss * 100.0,
        ));
        if let (Some(error), Some(clean)) = (pressure.error_time, pressure.clean_time) {
            text.push_str(&format!(
                "\nYour errors took {:.0}s to play; your sound moves {:.0}s.",
                seconds(error),
                seconds(clean)
            ));
        }
        text.push_str(if pressure.is_significant() {
            "\nYour mistakes really are concentrated in the moves you rush (p < 0.05). Slowing down would cost you less than studying more tactics."
        } else {
            "\nNo reliable link between speed and error in this game."
        });
    }
    text
}

/// Standard piece values, used only for the material readout.
/// Replay one move onto a FEN, giving the position the player actually faced.
/// Opt-in tracing, so a move that does not happen can be explained rather than
/// guessed at. Enable with `OMACHESS_TRACE=1`.
fn trace(message: &str) {
    if std::env::var_os("OMACHESS_TRACE").is_some() {
        eprintln!("omachess: {message}");
    }
}

/// The control at a position in the picker. The list is the presets followed
/// by "Untimed", so the last entry deliberately has no control behind it.
fn control_at(index: usize) -> Option<omachess_core::clock::TimeControl> {
    omachess_core::clock::PRESETS
        .get(index)
        .map(|(_, control)| *control)
}

/// A promotion in the notation `find_move` disambiguates with.
pub(crate) fn promotion_uci(from: Square, to: Square, role: shakmaty::Role) -> String {
    format!(
        "{from}{to}{}",
        match role {
            shakmaty::Role::Rook => "r",
            shakmaty::Role::Bishop => "b",
            shakmaty::Role::Knight => "n",
            _ => "q",
        }
    )
}

/// Minutes and seconds, and tenths once it is nearly gone — which is when a
/// tenth is the difference between moving and flagging.
fn format_clock(left: std::time::Duration) -> String {
    let secs = left.as_secs();
    if secs >= 20 {
        format!("{}:{:02}", secs / 60, secs % 60)
    } else {
        format!("{:.1}", left.as_secs_f64())
    }
}

fn clear_list(list: &ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

/// The scoresheet, with the time spent on each of the player's own moves —
/// which is the same quantity the trainer measures.
fn format_moves(game: &Game, times: &[std::time::Duration]) -> String {
    let player_is_white = game.player() == Color::White;
    game.move_pairs()
        .into_iter()
        .enumerate()
        .map(|(index, (number, white, black))| {
            let stamp = |mine: bool, n: usize| -> String {
                if !mine {
                    return String::new();
                }
                times
                    .get(n)
                    .map(|d| format!(" ({:.0}s)", d.as_secs_f64()))
                    .unwrap_or_default()
            };
            let white_part = format!("{white}{}", stamp(player_is_white, index));
            match black {
                Some(black) => format!(
                    "{number}. {white_part}  {black}{}",
                    stamp(!player_is_white, index)
                ),
                None => format!("{number}. {white_part}"),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use omachess_core::clock::PRESETS;

    /// The picker holds the presets plus "Untimed" at the end, so the last
    /// index must fall through to no clock rather than off the end of the list
    /// or onto the wrong control.
    #[test]
    fn the_last_entry_in_the_picker_is_untimed() {
        for (index, (_, expected)) in PRESETS.iter().enumerate() {
            assert_eq!(control_at(index), Some(*expected), "preset {index}");
        }
        assert_eq!(control_at(PRESETS.len()), None, "Untimed has no clock");
        assert_eq!(control_at(PRESETS.len() + 100), None);
    }

    /// Under twenty seconds the display switches to tenths, because that is
    /// when a tenth is the difference between moving and flagging.
    #[test]
    fn the_clock_shows_tenths_only_when_it_is_nearly_gone() {
        use std::time::Duration;
        assert_eq!(format_clock(Duration::from_secs(600)), "10:00");
        assert_eq!(format_clock(Duration::from_secs(65)), "1:05");
        assert_eq!(format_clock(Duration::from_secs(20)), "0:20");
        assert_eq!(format_clock(Duration::from_millis(19_400)), "19.4");
        assert_eq!(format_clock(Duration::ZERO), "0.0");
    }
}
