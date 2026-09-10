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

use omachess_core::store::{DrillOrigin, Store};
use shakmaty::Square;

use crate::drill_view::DrillView;
use crate::endgame_view::EndgameView;
use crate::pieces::PieceSet;
use crate::play_view::PlayView;
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
        outcome: caught(body),
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

/// Run a check body, turning a panic into a failed check rather than an abort.
///
/// The whole suite runs inside GTK's `activate` handler, and a panic that
/// reaches that frame unwinds into C, where Rust may not unwind: the process
/// aborts and dumps core before a word of the report is printed. A single
/// `unwrap` on a `None` used to take the entire run with it and say nothing
/// about which check was holding the knife.
///
/// The body holds widgets and stores a panic may leave half-built, so this is
/// `AssertUnwindSafe`. That is honest here: every check builds its own store
/// and its own views, and a run that panicked is being read for the panic
/// rather than trusted afterwards.
fn caught(body: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).unwrap_or_else(|payload| {
        // The panic hook has already put the file and line on stderr; what the
        // report needs is the reason, beside the name of the check.
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic with no message".to_owned());
        Err(format!("panicked: {message}"))
    })
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

/// A puzzle whose solution takes two moves, so the path through a *continued*
/// line is exercised and not only the one that ends it.
///
/// Black plays f6, White answers Rb8+, Black's king steps to f7, White plays
/// Rb7+. Nothing forced about it — what matters is that the solver moves twice.
const TWO_MOVE: &str = "\
PuzzleId,FEN,Moves,Rating,RatingDeviation,Popularity,NbPlays,Themes,GameUrl,OpeningTags,DailyDate
selftst3,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,f7f6 b1b8 g8f7 b8b7,1150,75,90,1000,endgame,https://lichess.org/c,,
";

fn two_move_store() -> Result<Store, String> {
    let mut store = Store::in_memory().map_err(|e| e.to_string())?;
    omachess_core::ingest::ingest_csv(&mut store, TWO_MOVE.as_bytes(), 1100)
        .map_err(|e| e.to_string())?;
    Ok(store)
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
pub fn run(pieces: Option<Rc<PieceSet>>, filter: Option<&str>) -> bool {
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
        let trainer = Trainer::new(store.clone(), pieces.clone(), None);
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
        let trainer = Trainer::new(store.clone(), pieces.clone(), None);
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
                &DrillOrigin {
                    source: "https://lichess.org/a".to_string(),
                    played_at: chrono::Utc::now(),
                    ply: 40,
                    played: "Kh8".to_string(),
                    best: "Rb8".to_string(),
                    lost: 0.9,
                    phase: "middlegame".to_string(),
                    win_before: 0.85,
                    best_line: Vec::new(),
                    game_id: None,
                },
            )
                .map_err(|e| e.to_string())?;
        }
        let drills = DrillView::new(store.clone(), pieces.clone(), None);
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
                let drills = DrillView::new(store.clone(), pieces.clone(), engine);
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
    //
    // Beside the board, not across the window. The refusal used to be stated
    // twice — once in the status line and once as a thirty-point strip over
    // everything — for a thing that happens several times a puzzle.
    checks.push(check("a refused move says why, beside the board", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let trainer = Trainer::new(store, pieces.clone(), None);
        trainer.begin_solving();

        // A legal rook move that is not the answer.
        trainer.board().click(Square::B1);
        trainer.board().click(Square::B4);

        let said = trainer.status_text();
        expect(
            said.to_lowercase().contains("not the move"),
            &format!("a wrong move was not refused in words: {said:?}"),
        )?;
        expect(
            crate::announce::last().is_none(),
            "the refusal is still being shouted across the window as well",
        )
    }));

    checks.push(check("a finished drill announces the result", || {
        use crate::announce::{self, Tone};
        let store = Rc::new(RefCell::new(seeded_store()?));
        store
            .borrow()
            .record_drill_origin(
                "selftst1",
                &DrillOrigin {
                    source: "https://lichess.org/a".to_string(),
                    played_at: chrono::Utc::now(),
                    ply: 40,
                    played: "Kh8".to_string(),
                    best: "Rb8".to_string(),
                    lost: 0.9,
                    phase: "middlegame".to_string(),
                    win_before: 0.85,
                    best_line: Vec::new(),
                    game_id: None,
                },
            )
            .map_err(|e| e.to_string())?;
        let drills = DrillView::new(store, pieces.clone(), None);
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
        let store = Rc::new(RefCell::new(seeded_store()?));
        store
            .borrow()
            .record_drill_origin(
                "selftst1",
                &DrillOrigin {
                    source: "https://lichess.org/a".to_string(),
                    played_at: chrono::Utc::now(),
                    ply: 40,
                    played: "Kh8".to_string(),
                    best: "Rb8".to_string(),
                    lost: 0.9,
                    phase: "middlegame".to_string(),
                    win_before: 0.85,
                    best_line: Vec::new(),
                    game_id: None,
                },
            )
            .map_err(|e| e.to_string())?;
        let drills = DrillView::new(store, pieces.clone(), None);
        drills.reload();
        drills.begin();

        // A rook cannot go there.
        drills.board().click(Square::B1);
        drills.board().click(Square::C3);

        // Beside the board, where the rest of the exercise speaks, rather than
        // as a strip across the window.
        let said = drills.status_text();
        expect(
            said.to_lowercase().contains("not legal"),
            &format!("an illegal move was refused silently: {said:?}"),
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
        let play = PlayView::new(store, pieces.clone(), None);
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
        let play = PlayView::new(store, pieces.clone(), None);
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
        let play = PlayView::new(store, pieces.clone(), None);
        play.begin_game();
        play.board().drag(Square::D2, Square::D4);
        expect(
            play.moves_played() == 1,
            &format!("dragging d2-d4 played nothing. {}", play.describe_state()),
        )
    }));

    checks.push(check("the move just played is marked on the board", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let play = PlayView::new(store, pieces.clone(), None);
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
        let play = PlayView::new(store, pieces.clone(), None);
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
        let play = PlayView::new(store.clone(), pieces.clone(), engine);
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
        let endgames = EndgameView::new(store, pieces.clone(), None);
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
        let endgames = EndgameView::new(store, pieces.clone(), None);
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
        let endgames = EndgameView::new(store, pieces.clone(), None);
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
        let trainer = Trainer::new(store.clone(), pieces.clone(), None);
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
        let trainer = Trainer::new(store, pieces.clone(), None);
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
        let play = PlayView::new(store, pieces.clone(), engine);
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
        let endgames = EndgameView::new(store, pieces.clone(), engine);
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
        let play = PlayView::new(store, pieces.clone(), engine);
        play.begin_game();
        play.board().drag(Square::E2, Square::E4);
        play.give_up();
        // Said once, and in the tab's own status line rather than a block that
        // sat on the screen until the next game.
        expect(
            play.status_text().to_lowercase().contains("resign"),
            &format!(
                "a finished game did not say how it ended: {:?}",
                play.status_text()
            ),
        )?;
        // The list of flagged moves only appears when there is something to
        // flag, and a two-ply game has nothing. What always has to arrive is
        // the report itself, which is what turns a game into training material.
        // Anywhere on the panel, not one named label: the figures moved out of
        // the detail line into tiles, and a check pinned to where a number used
        // to live tests the layout rather than the thing.
        expect(
            pump(90, || {
                play.panel_labels().iter().any(|t| t.contains("Accuracy"))
            }),
            &format!(
                "the report never reached the screen, so the game was analysed \
                 for nobody. {}",
                play.describe_state()
            ),
        )
    }));

    // --- the clock, which is where this player's losses come from ----------
    checks.push(check("a timed game's clock runs down", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let play = PlayView::new(store, pieces.clone(), None);
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

    // The one thing to do after a losing game, and it was not on the screen.
    //
    // A real game: twenty-two moves, a rook lost to a knight fork, resigned
    // sixteen points down. The report said no blunders and offered nothing;
    // once the report was fixed it said two blunders and still offered
    // nothing, because the button was gated on the positions making clean
    // puzzles. Going back over your own game does not need a unique answer.
    checks.push(check("a game with a mistake offers it back to practise", || {
        let engine = omachess_core::engine::find_engine();
        expect(engine.is_some(), "no engine on PATH, so nothing to analyse")?;
        let store = Rc::new(RefCell::new(seeded_store()?));
        let play = PlayView::new(store, pieces.clone(), engine);
        play.begin_game();

        // The king walks out. 1.f3 and 2.g4 only lose if Black finds the mate,
        // and a capped engine does not always, so this failed about one run in
        // four; a king on h4 by move four is a catastrophe against any reply.
        for (from, to) in [
            (Square::F2, Square::F3),
            (Square::E1, Square::F2),
            (Square::F2, Square::G3),
            (Square::G3, Square::H4),
        ] {
            let before = play.moves_played();
            play.board().drag(from, to);
            pump(30, || play.moves_played() >= before + 2);
        }
        play.give_up();

        expect(
            pump(120, || {
                play.panel_labels().iter().any(|t| t.contains("Accuracy"))
            }),
            "the report never arrived, so nothing was checked",
        )?;

        // Asked of the analysis rather than of the panel's text, and at any
        // severity rather than blunders alone: a first version keyed on the
        // word "Blunders" and returned early when it read zero, so it passed
        // without ever reaching the assertion.
        let flagged = play.flagged_moves();
        expect(
            flagged > 0,
            "the king was walked to h4 and the review flagged nothing, so this \
             check cannot say anything",
        )?;
        expect(
            play.practise_offered(),
            &format!(
                "the review flagged {flagged} moves and the tab offers nothing \
                 to practise. {}",
                play.describe_state()
            ),
        )
    }));

    // The Play tab used to run its own copy of the exercise, and that copy
    // wrote nothing to the store: no attempt, no move log, no session. You
    // could work through the position where your game turned and the
    // application would afterwards have no idea you had ever seen it. There is
    // one exercise now, in the Drill tab, and the button hands the position to
    // it — writing it down on the way, or there would be nothing to hand.
    checks.push(check("practising a game's mistake reaches the drill queue", || {
        let engine = omachess_core::engine::find_engine();
        expect(engine.is_some(), "no engine on PATH, so no game to review")?;
        let store = Rc::new(RefCell::new(seeded_store()?));
        let play = PlayView::new(store.clone(), pieces.clone(), engine);

        let landed: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let record = landed.clone();
        play.connect_practise(move |id| *record.borrow_mut() = Some(id.to_owned()));

        // The king walks out. 1.f3 and 2.g4 depend on Black finding the mate,
        // and a capped engine does not always, which made this flaky; a king on
        // h4 by move four is a catastrophe against any reply at all.
        play.begin_game();
        for (from, to) in [
            (Square::F2, Square::F3),
            (Square::E1, Square::F2),
            (Square::F2, Square::G3),
            (Square::G3, Square::H4),
        ] {
            let before = play.moves_played();
            play.board().drag(from, to);
            pump(30, || play.moves_played() >= before + 2);
        }
        play.give_up();
        expect(
            pump(120, || {
                play.panel_labels().iter().any(|t| t.contains("Accuracy"))
            }),
            "the report never arrived, so there was nothing to hand over",
        )?;
        expect(
            play.flagged_moves() > 0,
            &format!(
                "the review flagged nothing after walking the king to h4, so \
                 this check cannot say anything. {}",
                play.describe_state()
            ),
        )?;

        let before = store
            .borrow()
            .drills_to_play(50)
            .map_err(|e| e.to_string())?
            .len();
        play.press_practise();

        let id = landed
            .borrow()
            .clone()
            .ok_or("pressing practise sent nothing to the drill tab")?;
        let after = store
            .borrow()
            .drills_to_play(50)
            .map_err(|e| e.to_string())?;
        expect(
            after.len() > before,
            "practising a position left no trace in the queue, which is the \
             whole reason the Play tab stopped keeping its own copy",
        )?;
        expect(
            after.iter().any(|(queued, _)| *queued == id),
            &format!("the position handed over is not the one queued: {id}"),
        )?;
        // And it carries what the exercise needs to say anything.
        let origin = store
            .borrow()
            .drill_origin(&id)
            .map_err(|e| e.to_string())?
            .ok_or("the handed-over position has no record of what was played")?;
        expect(
            !origin.played.is_empty() && !origin.best.is_empty(),
            "the position was queued without the move that was played",
        )
    }));

    // Finding the move and converting it are different facts about the same
    // position. Only the second was ever recorded, so seeing the move and
    // failing to win with it looked identical to grinding out a win from a
    // position you never understood — and the first is the half this teaches.
    checks.push(check("whether the move was found is recorded", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        store
            .borrow()
            .record_drill_origin(
                "selftst1",
                &DrillOrigin {
                    source: String::new(),
                    played_at: chrono::Utc::now(),
                    ply: 40,
                    played: "Kh8".to_owned(),
                    best: "Rb8".to_owned(),
                    lost: 0.34,
                    phase: "middlegame".to_owned(),
                    win_before: 0.82,
                    best_line: vec!["b1b8".to_owned()],
                    game_id: None,
                },
            )
            .map_err(|e| e.to_string())?;
        let drills = DrillView::new(store.clone(), pieces.clone(), None);
        drills.reload();
        expect(
            store
                .borrow()
                .drill_answer_record()
                .map_err(|e| e.to_string())?
                == (0, 0),
            "an answer was recorded before anything was answered",
        )?;

        // Found first time.
        drills.board().click(Square::B1);
        drills.board().click(Square::B8);
        let (asked, found) = store
            .borrow()
            .drill_answer_record()
            .map_err(|e| e.to_string())?;
        expect(
            (asked, found) == (1, 1),
            &format!("finding the move recorded {found} of {asked}")
        )?;

        // Two wrong answers are one failure to find it, not two. A first
        // version of this asserted that answering correctly twice recorded
        // once, which cannot happen — the exercise has moved on to the
        // play-out by then — so it tested nothing.
        let fresh = Rc::new(RefCell::new(seeded_store()?));
        fresh
            .borrow()
            .record_drill_origin(
                "selftst1",
                &DrillOrigin {
                    source: String::new(),
                    played_at: chrono::Utc::now(),
                    ply: 40,
                    played: "Kh8".to_owned(),
                    best: "Rb8".to_owned(),
                    lost: 0.34,
                    phase: "middlegame".to_owned(),
                    win_before: 0.82,
                    best_line: vec!["b1b8".to_owned()],
                    game_id: None,
                },
            )
            .map_err(|e| e.to_string())?;
        let missed = DrillView::new(fresh.clone(), pieces.clone(), None);
        missed.reload();
        for _ in 0..2 {
            missed.board().click(Square::B1);
            missed.board().click(Square::B4);
        }
        let (asked, found) = fresh
            .borrow()
            .drill_answer_record()
            .map_err(|e| e.to_string())?;
        expect(
            (asked, found) == (1, 0),
            &format!("two wrong answers recorded {found} found of {asked} asked"),
        )
    }));

    // Going back over your own game should let you see how the position arose,
    // not drop a FEN on the board. The moves are replayed from the standard
    // start and checked against the position the drill actually poses, so a
    // game seeded from an opening line offers no context rather than a board
    // showing something else.
    checks.push(check("a drill can be walked back through the game", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let game_id = store
            .borrow()
            .record_game(&omachess_core::store::GameRecord {
                // 1.e4 e5 2.Nf3 Nc6 3.Bc4 Nf6 — six plies, then the mistake.
                moves_uci: "e2e4 e7e5 g1f3 b8c6 f1c4 g8f6".to_owned(),
                played_at: chrono::Utc::now(),
                player_white: true,
                opponent_elo: 1400,
                result: "lost".to_owned(),
                moves: 6,
                accuracy: 70.0,
                mean_loss: 0.1,
                blunders: 1,
                mistakes: 0,
                inaccuracies: 0,
                source: String::new(),
                player: String::new(),
                opening: String::new(),
                book_plies: 0,
                time_control: String::new(),
                pressure_moves: 0,
                pressure_blunders: 0,
                phases: [omachess_core::store::PhaseLoss::UNKNOWN; 3],
            })
            .map_err(|e| e.to_string())?;

        // The puzzle poses the position after those six plies: its own FEN is
        // one move earlier, plus the move that reached it.
        let puzzle = omachess_core::puzzle::Puzzle {
            id: "walkback".to_owned(),
            fen: "r1bqkbnr/pppp1ppp/2n5/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R b KQkq - 3 3".to_owned(),
            moves: vec!["g8f6".to_owned(), "f3g5".to_owned()],
            rating: 1200,
            rating_deviation: 0,
            popularity: 0,
            nb_plays: 0,
            themes: vec!["middlegame".to_owned()],
            game_url: String::new(),
            opening_tags: Vec::new(),
        };
        store
            .borrow_mut()
            .insert_puzzles(std::slice::from_ref(&puzzle))
            .map_err(|e| e.to_string())?;
        store
            .borrow()
            .record_drill_origin(
                "walkback",
                &DrillOrigin {
                    source: String::new(),
                    played_at: chrono::Utc::now(),
                    ply: 6,
                    played: "Bc5".to_owned(),
                    best: "Nxe4".to_owned(),
                    lost: 0.3,
                    phase: "opening".to_owned(),
                    win_before: 0.7,
                    best_line: vec!["f3g5".to_owned()],
                    game_id: Some(game_id),
                },
            )
            .map_err(|e| e.to_string())?;

        let drills = DrillView::new(store.clone(), pieces.clone(), None);
        drills.focus("walkback");

        let (at, pivot, len) = drills.navigation();
        expect(
            pivot == 6,
            &format!("the six moves before the mistake were not found: pivot {pivot}"),
        )?;
        expect(
            at == pivot,
            &format!("the exercise did not open on the mistake: at {at}, pivot {pivot}"),
        )?;
        expect(len >= 6, &format!("nothing to walk: {len} moves"))?;

        // Back goes into the game.
        drills.step_back();
        drills.step_back();
        let (at, _, _) = drills.navigation();
        expect(at == 4, &format!("stepping back twice reached {at}, wanted 4"))?;

        // Forward stops at the mistake: the answer is not on the axis yet.
        for _ in 0..5 {
            drills.step_forward();
        }
        let (at, pivot, _) = drills.navigation();
        expect(
            at == pivot,
            &format!("stepping forward walked past the mistake to {at} of {pivot}"),
        )?;

        // And the case the replay check exists for: a position whose game does
        // not lead to it — a game seeded from an opening line, or a stale link.
        // Offering no context is right; a board showing some other game is not.
        let wrong = omachess_core::puzzle::Puzzle {
            id: "mismatch".to_owned(),
            fen: "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1".to_owned(),
            moves: vec!["a2a3".to_owned(), "a7a6".to_owned()],
            ..puzzle.clone()
        };
        store
            .borrow_mut()
            .insert_puzzles(std::slice::from_ref(&wrong))
            .map_err(|e| e.to_string())?;
        store
            .borrow()
            .record_drill_origin(
                "mismatch",
                &DrillOrigin {
                    source: String::new(),
                    played_at: chrono::Utc::now(),
                    ply: 6,
                    played: "a3".to_owned(),
                    best: "e4".to_owned(),
                    lost: 0.9,
                    phase: "opening".to_owned(),
                    win_before: 0.7,
                    best_line: Vec::new(),
                    // Points at the game above, whose six moves reach a
                    // completely different position.
                    game_id: Some(game_id),
                },
            )
            .map_err(|e| e.to_string())?;
        drills.focus("mismatch");
        let (_, pivot, len) = drills.navigation();
        expect(
            pivot == 0 && len == 0,
            &format!(
                "a game that does not lead to this position was offered as its \
                 context: pivot {pivot}, {len} moves"
            ),
        )
    }));

    // Asking to practise a position must land on that position. It selected
    // the right one and then the tab switch rebuilt the list, which jumped
    // back to the worst mistake on record — so pressing the button sent you to
    // some other game entirely.
    checks.push(check("the drill opens on the position that was asked for", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let mut ids = Vec::new();
        // Three positions, worst first in the queue. The one to open is the
        // least costly, so landing on the top of the list is unmistakable.
        for (n, lost) in [("worst", 0.9_f64), ("middling", 0.5), ("wanted", 0.1)] {
            let puzzle = omachess_core::puzzle::Puzzle {
                id: n.to_owned(),
                fen: "6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1".to_owned(),
                moves: vec!["g8h8".to_owned(), "b1b8".to_owned()],
                rating: 1200,
                rating_deviation: 0,
                popularity: 0,
                nb_plays: 0,
                themes: vec!["middlegame".to_owned()],
                game_url: String::new(),
                opening_tags: Vec::new(),
            };
            store
                .borrow_mut()
                .insert_puzzles(std::slice::from_ref(&puzzle))
                .map_err(|e| e.to_string())?;
            store
                .borrow()
                .record_drill_origin(
                    n,
                    &DrillOrigin {
                        source: String::new(),
                        played_at: chrono::Utc::now(),
                        ply: 40,
                        played: "Kh8".to_owned(),
                        best: "Rb8".to_owned(),
                        lost,
                        phase: "middlegame".to_owned(),
                        win_before: 0.8,
                        best_line: Vec::new(),
                        game_id: None,
                    },
                )
                .map_err(|e| e.to_string())?;
            ids.push(n);
        }

        let drills = DrillView::new(store, pieces.clone(), None);
        drills.focus("wanted");
        expect(
            drills.showing().as_deref() == Some("wanted"),
            &format!("focus opened {:?}, not the position asked for", drills.showing()),
        )?;

        // And a rebuild — which is what showing the tab does — keeps it.
        drills.reload();
        expect(
            drills.showing().as_deref() == Some("wanted"),
            &format!(
                "rebuilding the list threw the choice away and opened {:?}",
                drills.showing()
            ),
        )
    }));

    // A mode that exists in the code and not on the screen is not a mode.
    // Board vision was built, committed, and reported as done while nothing
    // was running to show it.
    checks.push(check("the Train tab offers every way to train", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let trainer = Trainer::new(store, pieces.clone(), None);
        expect(
            trainer.mode_control_visible(),
            "the mode control is not on screen",
        )?;
        let offered = trainer.modes_offered();
        // Named rather than counted, so adding a mode does not break this and
        // dropping one does.
        for wanted in ["Learn", "Repeat", "vision", "Calculate"] {
            expect(
                offered.iter().any(|m| m.contains(wanted)),
                &format!("{wanted} is not one of the choices: {offered:?}"),
            )?;
        }
        Ok(())
    }));

    // "It's easy and doesn't feel like learning." It started at "is f6 light
    // or dark", which for someone who already knows the board is a quiz, and
    // twenty of them before anything interesting happens is how a ladder gets
    // abandoned. It starts at the hard end and falls to where it hurts.
    checks.push(check("board vision starts hard and gives way", || {
        use omachess_core::vision::Rung;
        let store = Rc::new(RefCell::new(seeded_store()?));
        let trainer = Trainer::new(store.clone(), pieces.clone(), None);
        trainer.choose_mode("vision");

        let opening = store.borrow().vision_rung().map_err(|e| e.to_string())?;
        expect(
            opening == Rung::opening_rung().key(),
            &format!("board vision opened on {opening:?}, not the hard end"),
        )?;

        // Get them wrong until it gives way. The gate is five, so this must
        // not take twenty.
        for _ in 0..omachess_core::vision::FALL_AFTER {
            expect(
                pump(10, || !trainer.vision_prompt().is_empty()),
                "no drill was dealt to answer",
            )?;
            if let Some(side) = trainer.vision_side_for(false) {
                trainer.press_vision(side);
            } else {
                // A clicking rung: commit nothing, which is wrong.
                trainer.press_vision_check();
            }
        }
        let after = store.borrow().vision_rung().map_err(|e| e.to_string())?;
        expect(
            after != opening,
            &format!("five wrong answers left it on {after:?}, which is a wall"),
        )
    }));

    // --- calculate: the line before the board moves ------------------------
    //
    // The puzzle trainer answers one move at a time, immediately, which lets a
    // puzzle be finished without ever being calculated. Here the board is
    // frozen and the whole line is written out first.
    checks.push(check("a written line is graded and the board is frozen", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let trainer = Trainer::new(store.clone(), pieces.clone(), None);
        trainer.choose_mode("calculate");
        expect(
            trainer.calculating(),
            "calculate mode put up no position to calculate",
        )?;

        // Touching the board must do nothing at all: reaching for a piece to
        // see what happens is the habit this mode exists to break. Asserted as
        // "nothing was graded", which is the part that can actually go wrong —
        // asserting the drill still exists passed even with the freeze removed.
        trainer.board().click(Square::B1);
        trainer.board().click(Square::B8);
        expect(
            trainer.calculating(),
            "the board moved while a line was being written",
        )?;
        expect(
            store
                .borrow()
                .calculation_record()
                .map_err(|e| e.to_string())?
                .0
                == 0,
            "clicking the board graded something before a line was committed",
        )?;

        let answer = trainer.calculation_answer();
        expect(!answer.is_empty(), "the position has no line to write")?;

        trainer.write_line(&answer.join(" "));
        trainer.press_commit();

        let (lines, mean) = store
            .borrow()
            .calculation_record()
            .map_err(|e| e.to_string())?;
        expect(
            lines == 1,
            &format!("committing a line recorded {lines} of them"),
        )?;
        expect(
            mean >= answer.len() as f64,
            &format!(
                "the whole line was written and only {mean} plies were credited \
                 of {}",
                answer.len()
            ),
        )
    }));

    checks.push(check("a wrong line is credited only as far as it was right", || {
        // A two-move puzzle: a mate in one has no second ply to break at, and
        // the first version of this used one and so tested nothing.
        let store = Rc::new(RefCell::new(two_move_store()?));
        let trainer = Trainer::new(store.clone(), pieces.clone(), None);
        trainer.choose_mode("calculate");
        let answer = trainer.calculation_answer();
        expect(!answer.is_empty(), "the position has no line to write")?;

        // Two right, then a legal move that is not the line. Legal matters:
        // notation that will not parse is refused before the comparison is
        // reached, so a nonsense token tests the parser and not the grading.
        expect(
            answer.len() >= 3,
            &format!("this check needs a line of three plies, got {answer:?}"),
        )?;
        trainer.write_line(&format!("{} {} Rb2", answer[0], answer[1]));
        trainer.press_commit();

        let (_, mean) = store
            .borrow()
            .calculation_record()
            .map_err(|e| e.to_string())?;
        expect(
            (mean - 2.0).abs() < f64::EPSILON,
            &format!("two right plies then a wrong legal move was credited {mean}"),
        )?;
        // The number alone is weak — a miscount can land on it by accident —
        // so the report has to name the ply and quote what was written.
        let said = trainer.status_text();
        expect(
            said.contains("Ply 3") && said.contains("Rb2"),
            &format!("the report did not say where it broke: {said:?}"),
        )
    }));

    // Same rule as everywhere else: three minutes spent calculating is not a
    // three-minute solve, and `attempts` feeds every speed figure there is.
    checks.push(check("calculating never touches the puzzle record", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        let before = store.borrow().solved_count().map_err(|e| e.to_string())?;
        let trainer = Trainer::new(store.clone(), pieces.clone(), None);
        trainer.choose_mode("calculate");
        let answer = trainer.calculation_answer();
        trainer.write_line(&answer.join(" "));
        trainer.press_commit();

        let (lines, _) = store
            .borrow()
            .calculation_record()
            .map_err(|e| e.to_string())?;
        expect(lines > 0, "nothing was recorded, so nothing was tested")?;
        let after = store.borrow().solved_count().map_err(|e| e.to_string())?;
        expect(
            after == before,
            "calculating wrote a puzzle solve; those figures are the transfer \
             measure and must not contain written lines",
        )
    }));

    // --- the ladder below calculation --------------------------------------
    //
    // He cannot visualise: plays on instinct, reacts, would lose to a serious
    // player every time. The puzzle trainer answers every move immediately,
    // which trains exactly that. These rungs are answered by clicking, decided
    // by geometry, and — critically — kept out of the measurements the rest of
    // the application depends on.
    checks.push(check("board vision asks a question and takes an answer", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        // Pinned: this checks the recording, not where the ladder opens.
        store.borrow().set_vision_rung("colour").map_err(|e| e.to_string())?;
        let trainer = Trainer::new(store.clone(), pieces.clone(), None);
        trainer.choose_mode("vision");

        let prompt = trainer.vision_prompt();
        expect(
            !prompt.is_empty(),
            "board vision showed no question at all",
        )?;

        let side = trainer
            .vision_side_for(true)
            .ok_or("no drill was dealt to answer")?;
        trainer.press_vision(side);

        let (asked, right) = store
            .borrow()
            .vision_record("colour")
            .map_err(|e| e.to_string())?;
        expect(
            (asked, right) == (1, 1),
            &format!("a right answer recorded {right} right of {asked} asked"),
        )
    }));

    checks.push(check("a wrong answer in board vision is refused", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        store.borrow().set_vision_rung("colour").map_err(|e| e.to_string())?;
        let trainer = Trainer::new(store.clone(), pieces.clone(), None);
        trainer.choose_mode("vision");
        let side = trainer
            .vision_side_for(false)
            .ok_or("no drill was dealt to answer")?;
        trainer.press_vision(side);

        let (asked, right) = store
            .borrow()
            .vision_record("colour")
            .map_err(|e| e.to_string())?;
        expect(
            (asked, right) == (1, 0),
            &format!("a wrong answer recorded {right} right of {asked} asked"),
        )
    }));

    // The load-bearing one. `attempts` is one row per puzzle and feeds
    // `paired_solves` and `transfer_by_band`; a drill answered in two seconds
    // is not a puzzle solved in two seconds, and letting these in would wreck
    // the transfer measure the whole application rests on.
    checks.push(check("board vision never touches the puzzle record", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        store.borrow().set_vision_rung("colour").map_err(|e| e.to_string())?;
        let before = store.borrow().solved_count().map_err(|e| e.to_string())?;
        let trainer = Trainer::new(store.clone(), pieces.clone(), None);
        trainer.choose_mode("vision");

        for _ in 0..8 {
            // The next drill arrives on a timer, so wait for it rather than
            // guessing how long it takes.
            if !pump(10, || !trainer.vision_prompt().is_empty()) {
                break;
            }
            if let Some(side) = trainer.vision_side_for(true) {
                trainer.press_vision(side);
            }
        }

        let (asked, _) = store
            .borrow()
            .vision_record("colour")
            .map_err(|e| e.to_string())?;
        expect(asked > 0, "no vision answers were recorded, so nothing was tested")?;
        let after = store.borrow().solved_count().map_err(|e| e.to_string())?;
        expect(
            after == before,
            &format!(
                "board vision wrote {} puzzle solves; those figures are the \
                 transfer measure and must not contain drills",
                after - before
            ),
        )
    }));

    // Four of the six rungs are answered by clicking squares rather than
    // pressing a button, and the board they are drawn on is usually not a
    // legal position at all — a lone knight has no kings.
    checks.push(check("a clicked rung takes the squares you click", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        store
            .borrow()
            .set_vision_rung("knight")
            .map_err(|e| e.to_string())?;
        let trainer = Trainer::new(store.clone(), pieces.clone(), None);
        trainer.choose_mode("vision");

        let wanted = trainer.vision_answer();
        expect(
            !wanted.is_empty(),
            "the knight rung asked for no squares at all",
        )?;

        for square in &wanted {
            trainer.board().click(*square);
        }
        expect(
            trainer.board().marks().len() == wanted.len(),
            &format!(
                "clicked {} squares and {} were marked",
                wanted.len(),
                trainer.board().marks().len()
            ),
        )?;
        trainer.press_vision_check();

        let (asked, right) = store
            .borrow()
            .vision_record("knight")
            .map_err(|e| e.to_string())?;
        expect(
            (asked, right) == (1, 1),
            &format!("clicking every right square scored {right} of {asked}"),
        )
    }));

    // --- the exercise, which is the point of the tab ----------------------
    //
    // It used to open on "Play it out against the engine. What you played last
    // time comes after" — the mistake withheld on the reasoning that walking in
    // blind is the exercise. It is not: going back over your own game is for
    // being shown what you did, and a wrong answer got no reply at all.
    checks.push(check("a drill says what you played before asking anything", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        store
            .borrow()
            .record_drill_origin(
                "selftst1",
                &DrillOrigin {
                    source: "https://lichess.org/a".to_owned(),
                    played_at: chrono::Utc::now(),
                    ply: 40,
                    played: "Kh8".to_owned(),
                    best: "Rb8".to_owned(),
                    lost: 0.34,
                    phase: "middlegame".to_owned(),
                    win_before: 0.82,
                    best_line: vec!["b1b8".to_owned()],
                    game_id: None,
                },
            )
            .map_err(|e| e.to_string())?;
        let drills = DrillView::new(store, pieces.clone(), None);
        drills.reload();

        let brief = drills.brief_text();
        expect(
            brief.contains("Kh8") && brief.contains("34"),
            &format!("the tab did not say what was played or what it cost: {brief:?}"),
        )?;
        expect(
            drills.asked_text().to_lowercase().contains("find the move"),
            &format!("the tab did not ask for anything: {:?}", drills.asked_text()),
        )
    }));

    checks.push(check("a wrong answer in a drill is explained", || {
        let engine = omachess_core::engine::find_engine();
        expect(engine.is_some(), "no engine on PATH, so nothing to explain")?;
        let store = Rc::new(RefCell::new(seeded_store()?));
        store
            .borrow()
            .record_drill_origin(
                "selftst1",
                &DrillOrigin {
                    source: String::new(),
                    played_at: chrono::Utc::now(),
                    ply: 40,
                    played: "Kh8".to_owned(),
                    best: "Rb8".to_owned(),
                    lost: 0.34,
                    phase: "middlegame".to_owned(),
                    win_before: 0.82,
                    best_line: vec!["b1b8".to_owned()],
                    game_id: None,
                },
            )
            .map_err(|e| e.to_string())?;
        let drills = DrillView::new(store, pieces.clone(), engine);
        drills.reload();

        // Legal, and not the answer.
        drills.board().click(Square::B1);
        drills.board().click(Square::B4);
        expect(
            pump(60, || {
                let said = drills.lesson_text();
                !said.is_empty() && said != "Looking at why…"
            }),
            &format!(
                "a wrong answer got no explanation: {:?}. {}",
                drills.lesson_text(),
                drills.describe_state()
            ),
        )?;
        expect(
            drills.lesson_text().starts_with("Rb4"),
            &format!(
                "the explanation did not name the move tried: {:?}",
                drills.lesson_text()
            ),
        )
    }));

    checks.push(check("the right answer in a drill is recognised", || {
        let store = Rc::new(RefCell::new(seeded_store()?));
        store
            .borrow()
            .record_drill_origin(
                "selftst1",
                &DrillOrigin {
                    source: String::new(),
                    played_at: chrono::Utc::now(),
                    ply: 40,
                    played: "Kh8".to_owned(),
                    best: "Rb8".to_owned(),
                    lost: 0.34,
                    phase: "middlegame".to_owned(),
                    win_before: 0.82,
                    best_line: vec!["b1b8".to_owned()],
                    game_id: None,
                },
            )
            .map_err(|e| e.to_string())?;
        let drills = DrillView::new(store, pieces.clone(), None);
        drills.reload();

        drills.board().click(Square::B1);
        drills.board().click(Square::B8);
        expect(
            drills.lesson_text().contains("Rb8"),
            &format!(
                "finding the move was not recognised: {:?} / {}",
                drills.lesson_text(),
                drills.describe_state()
            ),
        )?;
        expect(
            drills.asked_text().to_lowercase().contains("play it out"),
            &format!(
                "the exercise did not move on to converting it: {:?}",
                drills.asked_text()
            ),
        )
    }));

    // --- the engine as a teacher, and where it is not allowed --------------
    //
    // This is the load-bearing one. Every figure this application reports comes
    // from correct attempts alone, timed with a stopwatch the solver can see.
    // An engine consulted while that stopwatch is running would put its own
    // latency inside the measurement — and it would do so invisibly, because a
    // slower solve looks exactly like a slower solver. A wrong move sets the
    // attempt to failed, and only from that instant is the clock out of the
    // figures.
    checks.push(check("the engine is never asked while the clock counts", || {
        let engine = omachess_core::engine::find_engine();
        expect(engine.is_some(), "no engine on PATH, so nothing to ask")?;
        // A puzzle with two solver moves in it, so the check covers the move
        // that continues a line as well as the one that ends it. With a
        // mate-in-one the "continued" path is never reached, and a version of
        // this check built on one passed while the engine was being consulted
        // on every accepted move.
        let store = Rc::new(RefCell::new(two_move_store()?));
        let trainer = Trainer::new(store, pieces.clone(), engine);
        trainer.begin_solving();

        // Solve it correctly, start to finish. Nothing on this path may consult
        // the engine: the attempt counts, and its time is the measurement.
        trainer.board().click(Square::B1);
        trainer.board().click(Square::B8);
        expect(
            trainer.lesson_asks() == 0,
            &format!(
                "the engine was asked {} times on a move that continued the line",
                trainer.lesson_asks()
            ),
        )?;
        trainer.board().click(Square::B8);
        trainer.board().click(Square::B7);
        expect(
            trainer.lesson_asks() == 0,
            &format!(
                "the engine was asked {} times finishing an attempt that counted",
                trainer.lesson_asks()
            ),
        )
    }));

    checks.push(check("a wrong move is explained, not just refused", || {
        let engine = omachess_core::engine::find_engine();
        expect(engine.is_some(), "no engine on PATH, so nothing to explain")?;
        let store = Rc::new(RefCell::new(seeded_store()?));
        let trainer = Trainer::new(store, pieces.clone(), engine);
        trainer.begin_solving();

        // Legal, and not the answer. Rb1-b4 hangs nothing but solves nothing.
        trainer.board().click(Square::B1);
        trainer.board().click(Square::B4);
        expect(
            trainer.lesson_asks() == 1,
            &format!(
                "a wrong move asked the engine {} times",
                trainer.lesson_asks()
            ),
        )?;
        expect(
            pump(60, || {
                let said = trainer.lesson_text();
                !said.is_empty() && said != "Looking at why…"
            }),
            &format!(
                "the engine never said why the move was wrong: {:?}",
                trainer.lesson_text()
            ),
        )?;
        // It has to be about the move that was played, not about chess.
        let said = trainer.lesson_text();
        expect(
            said.starts_with("Rb4"),
            &format!("the explanation did not name the move played: {said:?}"),
        )
    }));

    // Play grew the same way Progress did: 287 words of explanation, and a
    // finished game reported as four paragraphs in a single label — accuracy
    // and loss in a sentence, the counts in a second, the weakest phase in a
    // third, the clock in two more. Every number in it was real and none could
    // be found. The same limit, enforced the same way.
    checks.push(check("the play panel states numbers, not paragraphs", || {
        const LIMIT: usize = 12;
        let engine = omachess_core::engine::find_engine();
        expect(engine.is_some(), "no engine on PATH, so no report to read")?;
        let store = Rc::new(RefCell::new(seeded_store()?));
        let play = PlayView::new(store, pieces.clone(), engine);
        play.begin_game();
        play.board().drag(Square::E2, Square::E4);
        pump(30, || play.moves_played() >= 2);
        play.give_up();
        // The report is the part that used to be an essay, so it has to be on
        // screen before this means anything.
        expect(
            pump(90, || {
                play.panel_labels().iter().any(|t| t.contains("Accuracy"))
            }),
            "the report never arrived, so nothing was checked",
        )?;

        let labels = play.panel_labels();
        let worst = labels
            .iter()
            .max_by_key(|text| text.split_whitespace().count())
            .expect("a label");
        let words = worst.split_whitespace().count();
        expect(
            words <= LIMIT,
            &format!("a label runs to {words} words, over the {LIMIT}-word limit: {worst:?}"),
        )
    }));

    // --- Progress: a measurement, not an essay ----------------------------
    //
    // The page carried eight hundred and seventy-seven words of explanation
    // around three real results, in thirteen sections, four of which reported
    // on tabs that are not even open. It was an essay with numbers hidden in
    // it. Prose is the failure mode here and it creeps back one helpful
    // sentence at a time, so the limit is checked rather than remembered.
    checks.push(check("the progress page states numbers, not paragraphs", || {
        use gtk4::prelude::*;
        const LIMIT: usize = 12;

        // Against the real database when one is pointed at, because a seeded
        // store has too little in it to draw most of the page, and the sections
        // that never render are exactly the ones that used to carry the prose.
        let store = match std::env::var("OMACHESS_SELFTEST_DB") {
            Ok(path) => Rc::new(RefCell::new(
                Store::open(std::path::Path::new(&path)).map_err(|e| e.to_string())?,
            )),
            Err(_) => Rc::new(RefCell::new(seeded_store()?)),
        };
        let trainer = Trainer::new(store.clone(), pieces.clone(), None);
        trainer.begin_solving();
        trainer.board().click(Square::B1);
        trainer.board().click(Square::B8);

        let progress = crate::progress_view::ProgressView::new();
        progress.refresh(&trainer.progress_data());

        // Every label on the page, however deeply nested.
        fn collect(widget: &gtk4::Widget, out: &mut Vec<String>) {
            if let Some(label) = widget.downcast_ref::<gtk4::Label>() {
                let text = label.text().to_string();
                if !text.is_empty() {
                    out.push(text);
                }
            }
            let mut child = widget.first_child();
            while let Some(node) = child {
                collect(&node, out);
                child = node.next_sibling();
            }
        }
        let mut labels = Vec::new();
        collect(progress.widget().upcast_ref::<gtk4::Widget>(), &mut labels);
        expect(!labels.is_empty(), "the progress page came back blank")?;

        let worst = labels
            .iter()
            .max_by_key(|text| text.split_whitespace().count())
            .expect("a label");
        let words = worst.split_whitespace().count();
        expect(
            words <= LIMIT,
            &format!(
                "a label runs to {words} words, over the {LIMIT}-word limit: {worst:?}"
            ),
        )?;

        let total: usize = labels.iter().map(|t| t.split_whitespace().count()).sum();
        expect(
            total <= 120,
            &format!("the page runs to {total} words across {} labels", labels.len()),
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
