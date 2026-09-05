//! Driving the real views in a real instance.
//!
//! Every defect this project has shipped lived in the interface layer, and none
//! of it was reachable from a unit test: the logic underneath was covered while
//! the wiring that connects a click to that logic was not. The bug that started
//! this — a board that answered a drag and ignored a click — is exactly that
//! shape, and it survived a full test suite because no test ever moved a piece.
//!
//! Synthetic keystrokes turned out not to reach GTK on this compositor, so this
//! does the next most honest thing: it builds the views a running application
//! builds, against a throwaway profile, and calls the handlers a click calls.
//! Same widgets, same store, same code path — driven directly rather than
//! through the compositor.

use std::cell::RefCell;
use std::rc::Rc;

use omachess_core::store::Store;
use shakmaty::Square;

use crate::drill_view::DrillView;
use crate::endgame_view::EndgameView;
use crate::pieces::PieceSet;
use crate::play_view::PlayView;
use crate::sound::Sounds;
use crate::study_view::StudyView;
use crate::trainer::Trainer;

/// One checked behaviour.
struct Check {
    name: &'static str,
    outcome: Result<(), String>,
    /// Filtered out rather than run. Reported separately so a filtered run can
    /// never be mistaken for a clean one.
    skipped: bool,
}

thread_local! {
    /// Only checks whose name contains this run. Set from the command line.
    static FILTER: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn wanted(name: &str) -> bool {
    FILTER.with(|filter| match filter.borrow().as_deref() {
        Some(want) => name.contains(want),
        None => true,
    })
}

fn check(name: &'static str, body: impl FnOnce() -> Result<(), String>) -> Check {
    if !wanted(name) {
        return Check {
            name,
            outcome: Ok(()),
            skipped: true,
        };
    }
    Check {
        name,
        outcome: body(),
        skipped: false,
    }
}

/// Give a widget a real size, so anything that reads its allocation sees one.
///
/// The promotion picker is positioned from the destination square's
/// allocation, so a board that was never allocated cannot be checked at all —
/// it would take the "no board to draw on" path and look like a pass.
fn allocate(widget: &impl gtk4::prelude::IsA<gtk4::Widget>, side: i32) {
    use gtk4::prelude::*;
    let widget = widget.as_ref();
    widget.set_size_request(side, side);
    widget.allocate(side, side, -1, None);
}

fn expect(condition: bool, complaint: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(complaint.to_owned())
    }
}

const CORPUS: &str = "\
PuzzleId,FEN,Moves,Rating,RatingDeviation,Popularity,NbPlays,Themes,GameUrl,OpeningTags,DailyDate
selftst1,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1150,75,90,1000,mateIn1,https://lichess.org/a,,
selftst2,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1160,75,90,1000,mateIn1,https://lichess.org/b,,
";

/// A short master game, written out so the Study tab has something real to
/// open. Anderssen–Kieseritzky, the Immortal, cut at move eight.
const SAMPLE_PGN: &str = "\
[Event \"London\"]
[Site \"London ENG\"]
[Date \"1851.06.21\"]
[White \"Anderssen, Adolf\"]
[Black \"Kieseritzky, Lionel\"]
[Result \"1-0\"]

1. e4 e5 2. f4 exf4 3. Bc4 Qh4+ 4. Kf1 b5 5. Bxb5 Nf6 6. Nf3 Qh6 7. d3 Nh5
8. Nh4 Qg5 1-0
";

/// Put the sample game somewhere the Study tab can open it from.
fn write_pgn() -> Result<std::path::PathBuf, String> {
    let path = std::env::temp_dir().join("omachess-selftest.pgn");
    std::fs::write(&path, SAMPLE_PGN).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Run the main loop until `ready` says so, or the deadline passes.
///
/// Engine replies arrive through the same timer the window uses, so nothing
/// involving the engine can be checked without letting the loop turn.
fn pump(seconds: u64, ready: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let context = gtk4::glib::MainContext::default();
    while std::time::Instant::now() < deadline {
        while context.iteration(false) {}
        if ready() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    ready()
}

fn seeded_store() -> Result<Store, String> {
    let mut store = Store::in_memory().map_err(|e| e.to_string())?;
    omachess_core::ingest::ingest_csv(&mut store, CORPUS.as_bytes(), 1100)
        .map_err(|e| e.to_string())?;
    Ok(store)
}

/// Build the views a running application builds and put them through the
/// motions a person would. Returns whether everything held.
/// `filter` runs only the checks whose name contains it, which is how a check
/// gets proved: break the thing it watches, run that one check, and see it go
/// red. A check that has never failed is decoration.
pub fn run(pieces: Option<Rc<PieceSet>>, sounds: Rc<Sounds>, filter: Option<&str>) -> bool {
    FILTER.with(|slot| *slot.borrow_mut() = filter.map(str::to_owned));
    let mut checks = Vec::new();

    // The window mounts this over everything; without it every announcement
    // goes nowhere and the checks below would pass on an application that says
    // nothing at all.
    let overlay = gtk4::Overlay::new();
    crate::announce::install(&overlay);

    // --- the trainer: click a piece, click where it goes ------------------
    checks.push(check("a puzzle can be solved by clicking", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let trainer = Trainer::new(store.clone(), pieces.clone(), sounds.clone());
        trainer.begin_solving();
        expect(trainer.solving(), "the trainer did not start solving")?;

        // Through the board, not the handler: a handler that was never
        // connected is the bug worth catching, and calling it directly cannot
        // see that.
        trainer.board().click(Square::B1);
        trainer.board().click(Square::B8);

        let solved = store.borrow().solved_count().map_err(|e| e.to_string())?;
        expect(solved == 1, "solving the puzzle recorded nothing")
    }));

    checks.push(check("a wrong move is refused, not accepted", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let trainer = Trainer::new(store.clone(), pieces.clone(), sounds.clone());
        trainer.begin_solving();

        // A legal but wrong rook move.
        trainer.board().click(Square::B1);
        trainer.board().click(Square::B4);
        expect(
            trainer.solving(),
            "a wrong move ended the attempt instead of being refused",
        )?;
        let solved = store.borrow().solved_count().map_err(|e| e.to_string())?;
        expect(solved == 0, "a wrong move was recorded as a solve")
    }));

    // --- the drill: the board that ignored clicks -------------------------
    checks.push(check("a drill position accepts a click", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        {
            let store = store.borrow();
            // A position taken from a game: White to move, Rb1-b8 mates.
            store
                .record_drill_origin(
                    "selftst1",
                    "https://lichess.org/a",
                    chrono::Utc::now(),
                    40,
                    "Kh8",
                    "Rb8",
                    0.9,
                    "middlegame",
                    0.85,
                )
                .map_err(|e| e.to_string())?;
        }
        let drills = DrillView::new(store.clone(), pieces.clone(), sounds.clone(), None);
        drills.reload();
        drills.begin();

        // Two clicks through the board. Before the fix this did nothing at
        // all and said nothing, because nothing was listening.
        drills.board().click(Square::B1);
        drills.board().click(Square::B8);

        let (attempts, _) = store
            .borrow()
            .drill_playout_record()
            .map_err(|e| e.to_string())?;
        expect(
            attempts == 1,
            "clicking through a drill recorded no attempt — the board is \
             ignoring clicks again",
        )
    }));

    // --- a real drill, a real engine, a real database ---------------------
    //
    // The checks above use a fabricated position and no engine, which is
    // exactly why they kept passing while the tab was unusable. This one opens
    // whatever database it is pointed at, takes the first drill on offer,
    // plays the move that answers it, and waits for the engine to reply.
    if let Ok(path) = std::env::var("OMACHESS_SELFTEST_DB") {
        checks.push(check(
            "a real drill plays out against a real engine",
            || {
                let store = Store::open(std::path::Path::new(&path)).map_err(|e| e.to_string())?;
                let offered = store.drills_to_play(1).map_err(|e| e.to_string())?;
                let (id, _) = offered
                    .first()
                    .ok_or("no drill positions on offer")?
                    .clone();
                let puzzle = store
                    .puzzle(&id)
                    .map_err(|e| e.to_string())?
                    .ok_or("the puzzle behind the drill is missing")?;
                // The answer is the second move of the stored line: the opponent
                // moves, then the player replies.
                let answer = puzzle
                    .moves
                    .get(1)
                    .cloned()
                    .ok_or("the drill has no answer stored")?;
                let from: Square = answer[0..2].parse().map_err(|_| "unreadable answer")?;
                let to: Square = answer[2..4].parse().map_err(|_| "unreadable answer")?;

                let engine = omachess_core::engine::find_engine();
                expect(engine.is_some(), "no engine on PATH, so nothing to play")?;

                let store = Rc::new(RefCell::new(store));
                let drills = DrillView::new(store.clone(), pieces.clone(), sounds.clone(), engine);
                drills.reload();
                drills.begin();

                let before = drills.moves_played();
                drills.board().click(from);
                let picked = drills.board().selected();
                expect(
                    picked == Some(from),
                    &format!(
                        "the first click did not pick up the piece on {from}: \
                     selection is {picked:?}. State: {}",
                        drills.describe_state()
                    ),
                )?;
                drills.board().click(to);
                let after = drills.moves_played();
                expect(
                    after > before,
                    &format!(
                        "clicking {from}-{to} played nothing ({before} -> {after}). State: {}",
                        drills.describe_state()
                    ),
                )?;

                // Let the engine answer. It replies through the same timer the
                // window uses, so the loop has to run for it to arrive.
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                let context = gtk4::glib::MainContext::default();
                while std::time::Instant::now() < deadline && drills.moves_played() == after {
                    while context.iteration(false) {}
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                expect(
                    drills.moves_played() > after,
                    "the engine never replied, so the position is stuck after one move",
                )
            },
        ));
    }

    // --- what the board says, now that it no longer flashes red -----------
    checks.push(check("a refused move says why, across the window", || {
        use crate::announce::{self, Tone};
        let store = Rc::new(RefCell::new(seeded_store()?));
        let trainer = Trainer::new(store, pieces.clone(), sounds.clone());
        trainer.begin_solving();
        announce::clear();

        // A legal rook move that is not the answer.
        trainer.board().click(Square::B1);
        trainer.board().click(Square::B4);

        let said = announce::last().ok_or("nothing was announced for a wrong move")?;
        expect(
            said.0 == Tone::Rejected,
            &format!("a wrong move was announced as {:?}", said.0),
        )?;
        expect(!said.1.is_empty(), "the rejection had no words in it")
    }));

    checks.push(check("a finished drill announces the result", || {
        use crate::announce::{self, Tone};
        let store = Rc::new(RefCell::new(seeded_store()?));
        store
            .borrow()
            .record_drill_origin(
                "selftst1",
                "https://lichess.org/a",
                chrono::Utc::now(),
                40,
                "Kh8",
                "Rb8",
                0.9,
                "middlegame",
                0.85,
            )
            .map_err(|e| e.to_string())?;
        let drills = DrillView::new(store, pieces.clone(), sounds.clone(), None);
        drills.reload();
        drills.begin();
        announce::clear();

        // Rb1-b8 is mate, which ends the position.
        drills.board().click(Square::B1);
        drills.board().click(Square::B8);

        let said = announce::last().ok_or("a finished drill announced nothing")?;
        expect(
            matches!(said.0, Tone::Won | Tone::Lost | Tone::Drawn),
            &format!("a result was announced as {:?}", said.0),
        )?;
        expect(!said.1.is_empty(), "the result had no words in it")
    }));

    checks.push(check("an illegal move is refused in words", || {
        use crate::announce::{self, Tone};
        let store = Rc::new(RefCell::new(seeded_store()?));
        store
            .borrow()
            .record_drill_origin(
                "selftst1",
                "https://lichess.org/a",
                chrono::Utc::now(),
                40,
                "Kh8",
                "Rb8",
                0.9,
                "middlegame",
                0.85,
            )
            .map_err(|e| e.to_string())?;
        let drills = DrillView::new(store, pieces.clone(), sounds.clone(), None);
        drills.reload();
        drills.begin();
        announce::clear();

        // A rook cannot go there.
        drills.board().click(Square::B1);
        drills.board().click(Square::C3);

        let said = announce::last().ok_or("an illegal move was refused silently")?;
        expect(
            said.0 == Tone::Rejected,
            &format!("an illegal move was announced as {:?}", said.0),
        )
    }));

    // --- promotion: the question that was never asked ---------------------
    checks.push(check("a promotion offers a choice", || {
        use shakmaty::fen::Fen;
        use shakmaty::{CastlingMode, Chess};
        let position: Chess = "8/P7/8/8/8/8/8/K6k w - - 0 1"
            .parse::<Fen>()
            .map_err(|e| e.to_string())?
            .into_position(CastlingMode::Standard)
            .map_err(|e| e.to_string())?;
        let choices = omachess_core::game::promotion_choices(&position, Square::A7, Square::A8);
        expect(
            choices.len() == 4,
            "a promoting pawn was not offered every piece",
        )
    }));

    // The question was asked and nobody could see it: a `GtkPopover` with
    // autohide dismisses itself the instant it fails to take a grab, which it
    // does when it is raised out of the drag gesture that asked for it. The
    // pawn stayed put with nothing on screen to say why, and the check above
    // passed the whole time, because it only ever asked the rules. This one
    // asks the board.
    checks.push(check("the promotion picker appears and can be clicked", || {
        use shakmaty::Role;

        let board = crate::board::BoardView::new(None);
        allocate(board.widget(), 480);

        let picked = Rc::new(std::cell::Cell::new(None));
        let record = picked.clone();
        board.ask_promotion(
            Square::A8,
            true,
            &[Role::Queen, Role::Rook, Role::Bishop, Role::Knight],
            move |role| record.set(Some(role)),
        );
        expect(
            board.promotion_showing(),
            "the promotion picker never became visible",
        )?;
        expect(
            board.promotion_choice_count() == 4,
            "the promotion picker did not offer every piece",
        )?;

        board.click_promotion_choice(Role::Knight)?;
        expect(
            picked.get() == Some(Role::Knight),
            "clicking a promotion choice reported nothing",
        )?;
        expect(
            !board.promotion_showing(),
            "the promotion picker stayed up after a choice",
        )
    }));


    // --- Play: the tab the engine is played on ----------------------------
    //
    // None of this was covered. The rules underneath were, and the rules were
    // never what broke: a view whose handler is not connected passes every
    // test in the core and does nothing at all on screen.
    checks.push(check("a game against the engine starts", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let play = PlayView::new(store, pieces.clone(), sounds.clone(), None);
        play.begin_game();
        expect(
            play.game_running(),
            &format!("no game was running after Start. {}", play.describe_state()),
        )?;
        expect(
            play.moves_played() == 0,
            "a fresh game already had moves in it",
        )
    }));

    checks.push(check("a move clicked on the board is played", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let play = PlayView::new(store, pieces.clone(), sounds.clone(), None);
        play.begin_game();
        play.board().click(Square::E2);
        expect(
            play.board().selected() == Some(Square::E2),
            "the first click did not pick the pawn up",
        )?;
        play.board().click(Square::E4);
        expect(
            play.moves_played() == 1,
            &format!("clicking e2-e4 played nothing. {}", play.describe_state()),
        )
    }));

    checks.push(check("a move dragged on the board is played", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let play = PlayView::new(store, pieces.clone(), sounds.clone(), None);
        play.begin_game();
        play.board().drag(Square::D2, Square::D4);
        expect(
            play.moves_played() == 1,
            &format!("dragging d2-d4 played nothing. {}", play.describe_state()),
        )
    }));

    checks.push(check("the move just played is marked on the board", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let play = PlayView::new(store, pieces.clone(), sounds.clone(), None);
        play.begin_game();
        play.board().drag(Square::E2, Square::E4);
        expect(
            play.board().square_has_class(Square::E2, "last-move")
                && play.board().square_has_class(Square::E4, "last-move"),
            "the move just played was not highlighted, so the engine's reply \
             will be impossible to spot",
        )
    }));

    checks.push(check("the opening is named while it is being played", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let play = PlayView::new(store, pieces.clone(), sounds.clone(), None);
        play.begin_game();
        play.board().drag(Square::E2, Square::E4);
        expect(
            !play.opening_text().is_empty(),
            &format!(
                "the opening went unnamed after 1.e4. {}",
                play.describe_state()
            ),
        )
    }));

    // A resigned game is still a played game, and the play-quality measurement
    // is built out of exactly these records. The analysis behind one needs the
    // engine, so this is the first check that cannot run without it.
    checks.push(check("resigning ends the game and records it", || {
        let engine = omachess_core::engine::find_engine();
        expect(engine.is_some(), "no engine on PATH, so nothing to record")?;
        let store = Rc::new(RefCell::new(seeded_store()?));
        let play = PlayView::new(store.clone(), pieces.clone(), sounds.clone(), engine);
        play.begin_game();
        play.board().drag(Square::E2, Square::E4);
        play.give_up();
        expect(
            !play.game_running(),
            &format!(
                "the game ran on after a resignation. {}",
                play.describe_state()
            ),
        )?;
        // The review runs on the engine thread and lands through the view's
        // own timer, so the loop has to turn for the record to be written.
        let recorded = pump(90, || {
            store
                .borrow()
                .games()
                .map(|games| !games.is_empty())
                .unwrap_or(false)
        });
        expect(
            recorded,
            "a resigned game left no record behind, so it counts for nothing \
             in the only measurement that says how well you are playing",
        )
    }));

    // --- Endgames: the one part of chess with a settled answer -------------
    checks.push(check("an endgame starts and accepts a move", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let endgames = EndgameView::new(store, pieces.clone(), sounds.clone(), None);
        endgames.begin_attempt();
        // The first entry is king and pawn with the king in front: White plays
        // Kd6, and Kc6 is the one king move that is not next to Black's.
        endgames.board().click(Square::D6);
        endgames.board().click(Square::C6);
        expect(
            endgames.moves_played() == 1,
            &format!(
                "clicking Kd6-c6 played nothing. {}",
                endgames.describe_state()
            ),
        )
    }));

    checks.push(check("an endgame accepts a dragged move too", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let endgames = EndgameView::new(store, pieces.clone(), sounds.clone(), None);
        endgames.begin_attempt();
        endgames.board().drag(Square::D6, Square::C6);
        expect(
            endgames.moves_played() == 1,
            &format!(
                "dragging Kd6-c6 played nothing. {}",
                endgames.describe_state()
            ),
        )
    }));

    checks.push(check("the fifty-move countdown is on screen", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let endgames = EndgameView::new(store, pieces.clone(), sounds.clone(), None);
        endgames.begin_attempt();
        expect(
            !endgames.countdown_text().is_empty(),
            "the fifty-move countdown said nothing, which in an endgame is the \
             difference between a win and a draw",
        )
    }));

    // --- Study: stepping through somebody else's game ----------------------
    checks.push(check("a PGN opens and steps forward and back", || {
        let path = write_pgn()?;
        let store = Rc::new(RefCell::new(seeded_store()?));
        let study = StudyView::new(store, pieces.clone(), None);
        study.open_path(&path);
        expect(
            study.games_loaded() == 1,
            &format!("the PGN loaded {} games", study.games_loaded()),
        )?;
        expect(study.ply() == 0, "the game did not open at the start")?;
        study.go_forward();
        study.go_forward();
        expect(
            study.ply() == 2,
            &format!("stepping forward twice reached ply {}", study.ply()),
        )?;
        study.go_back();
        expect(
            study.ply() == 1,
            &format!("stepping back reached ply {}", study.ply()),
        )?;
        expect(
            study.scoresheet_text().contains("1."),
            &format!(
                "the scoresheet did not number the moves: {:?}",
                study.scoresheet_text()
            ),
        )
    }));

    // --- Progress: the point of the whole application ----------------------
    checks.push(check("the progress view draws from real data", || {
        use gtk4::prelude::*;
        let store = Rc::new(RefCell::new(seeded_store()?));
        let trainer = Trainer::new(store.clone(), pieces.clone(), sounds.clone());
        trainer.begin_solving();
        trainer.board().click(Square::B1);
        trainer.board().click(Square::B8);

        let progress = crate::progress_view::ProgressView::new();
        progress.refresh(&trainer.progress_data());
        let mut sections = 0;
        let mut child = progress.widget().first_child();
        while let Some(node) = child {
            sections += 1;
            child = node.next_sibling();
        }
        expect(
            sections > 0,
            "the progress view came back empty after a solve",
        )
    }));

    // --- the board's own vocabulary ---------------------------------------
    checks.push(check("mate is marked on the king that is mated", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let trainer = Trainer::new(store, pieces.clone(), sounds.clone());
        trainer.begin_solving();
        // Rb1-b8 is mate; the black king is on h8.
        trainer.board().click(Square::B1);
        trainer.board().click(Square::B8);
        expect(
            trainer.board().square_has_class(Square::H8, "mated"),
            "a mated king was not marked, which is the single most important \
             thing the board can say",
        )
    }));

    checks.push(check("the board can be turned round", || {
        let board = crate::board::BoardView::new(None);
        let white_view = board.grid_slot(Square::A1);
        board.set_orientation(shakmaty::Color::Black);
        let black_view = board.grid_slot(Square::A1);
        expect(
            white_view == (0, 7),
            &format!("a1 sat at {white_view:?} with White at the bottom"),
        )?;
        expect(
            black_view == (7, 0),
            &format!("a1 sat at {black_view:?} with Black at the bottom"),
        )
    }));

    checks.push(check("the keyboard cursor walks the board", || {
        let board = crate::board::BoardView::new(None);
        board.press_arrow(0, 0);
        let start = board.cursor().ok_or("an arrow key produced no cursor")?;
        board.press_arrow(1, 0);
        let moved = board.cursor().ok_or("the cursor disappeared")?;
        expect(
            moved != start,
            &format!("the cursor stayed on {start} when told to go right"),
        )
    }));

    // --- the engine, through the views rather than beside them -------------
    //
    // The engine has always been exercised directly. That proves Stockfish
    // works, which was never in doubt; what breaks is the path between it and
    // the board, and each of these waits on the same timer the window does.
    checks.push(check("the engine answers a move in Play", || {
        let engine = omachess_core::engine::find_engine();
        expect(engine.is_some(), "no engine on PATH, so nothing to answer")?;
        let store = Rc::new(RefCell::new(seeded_store()?));
        let play = PlayView::new(store, pieces.clone(), sounds.clone(), engine);
        play.begin_game();
        play.board().drag(Square::E2, Square::E4);
        expect(
            pump(30, || play.moves_played() >= 2),
            &format!(
                "the engine never replied, so the game is stuck. {}",
                play.describe_state()
            ),
        )
    }));

    checks.push(check("the engine defends an endgame", || {
        let engine = omachess_core::engine::find_engine();
        expect(engine.is_some(), "no engine on PATH, so nothing to defend")?;
        let store = Rc::new(RefCell::new(seeded_store()?));
        let endgames = EndgameView::new(store, pieces.clone(), sounds.clone(), engine);
        endgames.begin_attempt();
        endgames.board().drag(Square::D6, Square::C6);
        expect(
            pump(30, || endgames.moves_played() >= 2),
            &format!(
                "the defender never moved, so the endgame cannot be converted \
                 or drawn. {}",
                endgames.describe_state()
            ),
        )
    }));

    checks.push(check("the engine teaches the position in Study", || {
        let engine = omachess_core::engine::find_engine();
        expect(engine.is_some(), "no engine on PATH, so nothing to teach with")?;
        let path = write_pgn()?;
        let store = Rc::new(RefCell::new(seeded_store()?));
        let study = StudyView::new(store, pieces.clone(), engine);
        study.open_path(&path);
        study.go_forward();
        study.go_forward();
        // Not the evaluation label: that says "Move 2 of 16" before the engine
        // has searched a single node, so a check on it passes with the engine
        // unplugged. The variation is the only line that cannot be written
        // without one.
        expect(
            pump(60, || {
                let line = study.variation_text();
                !line.is_empty() && line != "Thinking…"
            }),
            &format!(
                "the engine gave no line for the position on the board, which \
                 is the entire point of the tab. {} variation {:?}",
                study.describe_state(),
                study.variation_text()
            ),
        )
    }));

    checks.push(check("a finished game puts its report on screen", || {
        let engine = omachess_core::engine::find_engine();
        expect(engine.is_some(), "no engine on PATH, so nothing to report")?;
        let store = Rc::new(RefCell::new(seeded_store()?));
        let play = PlayView::new(store, pieces.clone(), sounds.clone(), engine);
        play.begin_game();
        play.board().drag(Square::E2, Square::E4);
        play.give_up();
        expect(
            !play.banner_text().is_empty(),
            "a finished game said nothing about how it ended",
        )?;
        // The list of flagged moves only appears when there is something to
        // flag, and a two-ply game has nothing. What always has to arrive is
        // the report itself, which is what turns a game into training material.
        expect(
            pump(90, || play.detail_text().contains("Accuracy")),
            &format!(
                "the report never reached the screen, so the game was analysed \
                 for nobody. {} detail {:?}",
                play.describe_state(),
                play.detail_text()
            ),
        )
    }));

    // --- the clock, which is where this player's losses come from ----------
    checks.push(check("a timed game's clock runs down", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let play = PlayView::new(store, pieces.clone(), sounds.clone(), None);
        // The first entry is a real time control; the last is untimed.
        play.pick_time_control(0);
        play.begin_game();
        let first = play.clock_text();
        expect(
            !first.is_empty(),
            "a timed game showed no clock at all",
        )?;
        expect(
            pump(10, || play.clock_text() != first),
            &format!("the clock stayed on {first} and never moved"),
        )
    }));

    // --- sound: not whether it is audible, but whether there is anything ---
    checks.push(check("every sound cue has a clip to play", || {
        let missing = sounds.missing_clips();
        expect(
            missing.is_empty(),
            &format!(
                "these cues would play in silence: {}. Silence is \
                 indistinguishable from the sound being off, so nobody would \
                 ever report it",
                missing.join(", ")
            ),
        )
    }));

    // --- the plan: what the application tells you to do -------------------
    checks.push(check("the plan always has something to say", || {
        let store = seeded_store()?;
        let plan = omachess_core::plan::todays_plan(&store).map_err(|e| e.to_string())?;
        expect(!plan.is_empty(), "the plan came back empty")?;
        expect(
            plan.iter().all(|step| !step.why.is_empty()),
            "a step could not say why it was there",
        )
    }));

    let ran: Vec<&Check> = checks.iter().filter(|c| !c.skipped).collect();
    let failed = ran.iter().filter(|c| c.outcome.is_err()).count();
    let skipped = checks.len() - ran.len();
    println!("Self-test — {} checks", ran.len());
    for entry in &ran {
        match &entry.outcome {
            Ok(()) => println!("  pass  {}", entry.name),
            Err(why) => println!("  FAIL  {}\n        {why}", entry.name),
        }
    }
    if skipped > 0 {
        println!("\n{skipped} skipped by the filter.");
    }
    if failed == 0 {
        println!("\nEverything held.");
    } else {
        println!("\n{failed} of {} failed.", ran.len());
    }
    failed == 0
}
