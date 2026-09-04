//! End-to-end checks on the claim the project is built around.
//!
//! The central integrity property: solving new puzzles must never produce a
//! progress figure. Only re-solving a puzzle you have already solved can, and
//! then only when enough of them agree.

use chrono::{Duration, TimeZone, Utc};
use omachess_core::grade::RATING_FLOOR;
use omachess_core::ingest::ingest_csv;
use omachess_core::progress::{measured_improvement, MIN_PAIRS};
use omachess_core::session::{Session, Solve};
use omachess_core::store::Store;
use rs_fsrs::Rating;

const CSV: &str = "\
PuzzleId,FEN,Moves,Rating,RatingDeviation,Popularity,NbPlays,Themes,GameUrl,OpeningTags,DailyDate
p0000001,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1150,75,90,1000,mateIn1,https://lichess.org/a,,
p0000002,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1180,75,90,1000,fork,https://lichess.org/b,,
p0000003,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1120,75,90,1000,fork,https://lichess.org/c,,
p0000004,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1160,75,90,1000,pin,https://lichess.org/d,,
p0000005,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1140,75,90,1000,skewer,https://lichess.org/e,,
p0000006,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1170,75,90,1000,fork,https://lichess.org/f,,
p0000007,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1130,75,90,1000,pin,https://lichess.org/g,,
p0000008,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1190,75,90,1000,fork,https://lichess.org/h,,
";

const IDS: [&str; 8] = [
    "p0000001", "p0000002", "p0000003", "p0000004", "p0000005", "p0000006", "p0000007", "p0000008",
];

fn loaded_store() -> Store {
    let mut store = Store::in_memory().unwrap();
    ingest_csv(&mut store, CSV.as_bytes(), RATING_FLOOR).unwrap();
    store
}

/// Solve every puzzle once, taking `seconds` each, `day` days in.
fn solve_round(store: &mut Store, day: i64, seconds: i64, correct: bool) {
    let session = Session::new();
    let start = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
    for (index, id) in IDS.iter().enumerate() {
        let solve = Solve {
            puzzle_id: (*id).to_owned(),
            puzzle_rating: 1150,
            correct,
            elapsed: Duration::seconds(seconds),
        };
        // Spread within the day so ordering is unambiguous.
        let now = start + Duration::days(day) + Duration::minutes(index as i64);
        session.submit(store, &solve, now).unwrap();
    }
}

#[test]
fn solving_new_puzzles_never_produces_a_progress_figure() {
    let mut store = loaded_store();
    solve_round(&mut store, 0, 30, true);

    assert_eq!(store.solved_count().unwrap(), IDS.len() as u64);
    assert!(
        measured_improvement(&store).unwrap().is_none(),
        "a first pass through new puzzles is not evidence of improvement"
    );
}

#[test]
fn re_solving_the_same_puzzles_faster_is_measured_and_significant() {
    let mut store = loaded_store();
    solve_round(&mut store, 0, 40, true);
    solve_round(&mut store, 7, 16, true);

    let result = measured_improvement(&store).unwrap().expect("a measurement");
    assert_eq!(result.puzzles, IDS.len());
    assert_eq!(result.faster, IDS.len());
    assert!(
        (result.median_speedup - 2.5).abs() < 1e-6,
        "40s to 16s is 2.5x, got {}",
        result.median_speedup
    );
    assert!(result.is_significant(), "p was {}", result.p_value);
}

#[test]
fn re_solving_slower_is_reported_as_a_decline() {
    let mut store = loaded_store();
    solve_round(&mut store, 0, 16, true);
    solve_round(&mut store, 7, 40, true);

    let result = measured_improvement(&store).unwrap().expect("a measurement");
    assert_eq!(result.slower, IDS.len());
    assert!(result.median_speedup < 1.0);
    assert!(
        !result.is_significant(),
        "getting slower must never be reported as significant improvement"
    );
}

#[test]
fn failed_repeats_do_not_count_toward_speed() {
    let mut store = loaded_store();
    solve_round(&mut store, 0, 40, true);
    // A fast but wrong second attempt must not look like a fast solve.
    solve_round(&mut store, 7, 2, false);

    assert!(
        measured_improvement(&store).unwrap().is_none(),
        "wrong answers were counted as solves"
    );
    let (rate, attempts) = store.repeat_accuracy().unwrap();
    assert_eq!(attempts, IDS.len() as u32);
    assert!((rate - 0.0).abs() < 1e-9, "accuracy on repeat should be zero");
}

#[test]
fn measurement_needs_a_minimum_number_of_repeated_puzzles() {
    let mut store = loaded_store();
    let session = Session::new();
    let start = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();

    // Repeat only a handful — fewer than the floor.
    for round in 0..2 {
        for (index, id) in IDS.iter().take(MIN_PAIRS - 1).enumerate() {
            session
                .submit(
                    &mut store,
                    &Solve {
                        puzzle_id: (*id).to_owned(),
                        puzzle_rating: 1150,
                        correct: true,
                        elapsed: Duration::seconds(if round == 0 { 40 } else { 10 }),
                    },
                    start + Duration::days(round * 7) + Duration::minutes(index as i64),
                )
                .unwrap();
        }
    }
    assert!(measured_improvement(&store).unwrap().is_none());
}

#[test]
fn repeat_mode_serves_only_puzzles_already_solved() {
    let mut store = loaded_store();
    let session = Session::new();
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();

    session
        .submit(
            &mut store,
            &Solve {
                puzzle_id: "p0000003".into(),
                puzzle_rating: 1120,
                correct: true,
                elapsed: Duration::seconds(20),
            },
            now,
        )
        .unwrap();

    store.set_repeat_mode(true).unwrap();
    for _ in 0..8 {
        let served = session
            .next_puzzle(&store, now + Duration::days(30))
            .unwrap()
            .expect("repeat mode should serve something");
        assert_eq!(
            served.id, "p0000003",
            "repeat mode served a puzzle that had never been solved"
        );
    }
}

#[test]
fn learn_mode_serves_only_puzzles_not_seen_before() {
    let mut store = loaded_store();
    let session = Session::new();
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();

    session
        .submit(
            &mut store,
            &Solve {
                puzzle_id: "p0000003".into(),
                puzzle_rating: 1120,
                correct: true,
                elapsed: Duration::seconds(20),
            },
            now,
        )
        .unwrap();

    store.set_repeat_mode(false).unwrap();
    for _ in 0..8 {
        let served = session
            .next_puzzle(&store, now + Duration::days(30))
            .unwrap()
            .expect("learn mode should serve something");
        assert_ne!(
            served.id, "p0000003",
            "learn mode re-served a puzzle already solved"
        );
    }
}

#[test]
fn a_failure_still_schedules_a_prompt_return() {
    let mut store = loaded_store();
    let session = Session::new();
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();

    let outcome = session
        .submit(
            &mut store,
            &Solve {
                puzzle_id: "p0000001".into(),
                puzzle_rating: 1150,
                correct: false,
                elapsed: Duration::seconds(2),
            },
            now,
        )
        .unwrap();

    assert_eq!(outcome.grade, Rating::Again);
    assert!(outcome.due < now + Duration::days(1));
}

#[test]
fn the_personal_rating_starts_at_the_floor_and_rises_with_solves() {
    let mut store = loaded_store();
    assert_eq!(store.personal_rating().unwrap(), f64::from(RATING_FLOOR));
    solve_round(&mut store, 0, 20, true);
    assert!(store.personal_rating().unwrap() > f64::from(RATING_FLOOR));
}

/// The exact sequence the trainer performs, driven headlessly.
///
/// The GTK layer only draws and dispatches; every decision it makes is one of
/// these calls. Running the sequence here covers the loop without a display.
#[test]
fn the_full_training_loop_behaves_across_a_mode_switch() {
    use omachess_core::progress::measured_improvement;
    use omachess_core::puzzle::{Attempt, MoveOutcome};

    let mut store = loaded_store();
    let session = Session::new();
    let start = Utc.with_ymd_and_hms(2026, 5, 1, 9, 0, 0).unwrap();

    // Learn mode: work through unseen puzzles, solving each properly.
    store.set_repeat_mode(false).unwrap();
    let mut solved = Vec::new();
    for i in 0..IDS.len() {
        let now = start + Duration::minutes(i as i64 * 3);
        let puzzle = session
            .next_puzzle(&store, now)
            .unwrap()
            .expect("learn mode should always have something unseen");
        assert!(
            !solved.contains(&puzzle.id),
            "learn mode re-served {}",
            puzzle.id
        );

        // Solve it the way the board does: play the recorded answer.
        let mut attempt = Attempt::new(&puzzle).unwrap();
        let expected = attempt.expected().unwrap().to_owned();
        let mv = attempt.parse_move(&expected).unwrap();
        assert_eq!(attempt.play(&mv).unwrap(), MoveOutcome::Solved);

        session
            .submit(
                &mut store,
                &Solve {
                    puzzle_id: puzzle.id.clone(),
                    puzzle_rating: puzzle.rating,
                    correct: true,
                    elapsed: Duration::seconds(40),
                },
                now,
            )
            .unwrap();
        solved.push(puzzle.id);
    }

    assert_eq!(store.solved_count().unwrap(), IDS.len() as u64);
    assert!(
        measured_improvement(&store).unwrap().is_none(),
        "a first pass is never progress"
    );

    // Repeat mode, the next day: the same puzzles come back, solved faster.
    store.set_repeat_mode(true).unwrap();
    for i in 0..IDS.len() {
        let now = start + Duration::hours(30) + Duration::minutes(i as i64 * 3);
        let puzzle = session
            .next_puzzle(&store, now)
            .unwrap()
            .expect("repeat mode should serve solved puzzles");
        assert!(
            solved.contains(&puzzle.id),
            "repeat mode served an unseen puzzle: {}",
            puzzle.id
        );
        session
            .submit(
                &mut store,
                &Solve {
                    puzzle_id: puzzle.id.clone(),
                    puzzle_rating: puzzle.rating,
                    correct: true,
                    elapsed: Duration::seconds(16),
                },
                now,
            )
            .unwrap();
    }

    let result = measured_improvement(&store)
        .unwrap()
        .expect("a day later, the repeats count");
    assert!(result.median_speedup > 2.0, "40s to 16s is 2.5x");
    assert!(result.is_significant(), "p was {}", result.p_value);
}

/// Solving the same puzzle again within the session must never register as
/// progress, however much faster it was.
#[test]
fn a_same_session_repeat_is_recorded_but_never_measured() {
    let mut store = loaded_store();
    let session = Session::new();
    let start = Utc.with_ymd_and_hms(2026, 5, 1, 9, 0, 0).unwrap();

    for round in 0..2 {
        for (index, id) in IDS.iter().enumerate() {
            session
                .submit(
                    &mut store,
                    &Solve {
                        puzzle_id: (*id).to_owned(),
                        puzzle_rating: 1150,
                        correct: true,
                        elapsed: Duration::seconds(if round == 0 { 40 } else { 5 }),
                    },
                    start + Duration::minutes(round * 30 + index as i64),
                )
                .unwrap();
        }
    }

    assert_eq!(
        store.attempt_log().unwrap().len(),
        IDS.len() * 2,
        "both rounds must still be recorded"
    );
    assert!(
        measured_improvement(&store).unwrap().is_none(),
        "half an hour later is recall, not skill"
    );
}
