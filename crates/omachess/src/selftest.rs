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
use crate::pieces::PieceSet;
use crate::sound::Sounds;
use crate::trainer::Trainer;

/// One checked behaviour.
struct Check {
    name: &'static str,
    outcome: Result<(), String>,
}

fn check(name: &'static str, body: impl FnOnce() -> Result<(), String>) -> Check {
    Check {
        name,
        outcome: body(),
    }
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

fn seeded_store() -> Result<Store, String> {
    let mut store = Store::in_memory().map_err(|e| e.to_string())?;
    omachess_core::ingest::ingest_csv(&mut store, CORPUS.as_bytes(), 1100)
        .map_err(|e| e.to_string())?;
    Ok(store)
}

/// Build the views a running application builds and put them through the
/// motions a person would. Returns whether everything held.
pub fn run(pieces: Option<Rc<PieceSet>>, sounds: Rc<Sounds>) -> bool {
    let mut checks = Vec::new();

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

    let failed = checks.iter().filter(|c| c.outcome.is_err()).count();
    println!("Self-test — {} checks", checks.len());
    for entry in &checks {
        match &entry.outcome {
            Ok(()) => println!("  pass  {}", entry.name),
            Err(why) => println!("  FAIL  {}\n        {why}", entry.name),
        }
    }
    if failed == 0 {
        println!("\nEverything held.");
    } else {
        println!("\n{failed} of {} failed.", checks.len());
    }
    failed == 0
}
