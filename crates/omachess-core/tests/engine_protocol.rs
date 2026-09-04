//! Exercises the UCI layer against a real engine.
//!
//! Engines are not a build dependency, so these tests are skipped unless
//! `OMACHESS_TEST_ENGINE` points at one:
//!
//!     OMACHESS_TEST_ENGINE=/usr/bin/stockfish cargo test -p omachess-core

use std::path::PathBuf;
use std::time::Duration;

use omachess_core::engine::{Engine, Limit, Score, MIN_LIMITED_ELO};
use omachess_core::game::START_FEN;

fn engine() -> Option<Engine> {
    let path = PathBuf::from(std::env::var_os("OMACHESS_TEST_ENGINE")?);
    Some(Engine::spawn(&path).expect("engine should start"))
}

macro_rules! engine_or_skip {
    () => {
        match engine() {
            Some(engine) => engine,
            None => return,
        }
    };
}

#[test]
fn the_handshake_reports_a_name() {
    let engine = engine_or_skip!();
    assert!(!engine.id().is_empty(), "engine did not identify itself");
}

#[test]
fn the_opening_position_yields_a_legal_first_move() {
    let mut engine = engine_or_skip!();
    let analysis = engine
        .analyse(START_FEN, &[], Limit::Depth(8))
        .expect("analysis should succeed");

    let best = analysis.best_move.expect("engine returned no move");
    // Every legal first move is a pawn or knight move from rank 1 or 2.
    assert_eq!(best.len(), 4, "unexpected move format: {best}");
    assert!(
        matches!(&best[1..2], "1" | "2"),
        "first move must come from White's own half: {best}"
    );
    assert!(analysis.depth >= 1);
}

#[test]
fn a_forced_mate_is_seen_as_mate() {
    let mut engine = engine_or_skip!();
    // Black king on h8, White rook lifts to the eighth rank: mate in one.
    let analysis = engine
        .analyse(
            "6k1/5ppp/8/8/8/8/5PPP/1R4K1 w - - 0 1",
            &[],
            Limit::Depth(12),
        )
        .expect("analysis should succeed");
    assert_eq!(analysis.best_move.as_deref(), Some("b1b8"));
    assert!(
        matches!(analysis.score, Some(Score::Mate(n)) if n > 0),
        "expected a mate score, got {:?}",
        analysis.score
    );
}

#[test]
fn evaluations_are_signed_from_the_side_to_move() {
    let mut engine = engine_or_skip!();
    // White is a queen up, with each side to move in turn.
    let white_to_move = engine
        .analyse("4k3/8/8/8/8/8/8/3QK3 w - - 0 1", &[], Limit::Depth(10))
        .unwrap()
        .score
        .expect("a score");
    let black_to_move = engine
        .analyse("4k3/8/8/8/8/8/8/3QK3 b - - 0 1", &[], Limit::Depth(10))
        .unwrap()
        .score
        .expect("a score");

    // A shallow search reports a large centipawn score rather than mate, so the
    // thresholds test the sign and the asymmetry, not a particular magnitude.
    assert!(
        white_to_move.win_chance() > 0.8,
        "the side a queen up should be winning: {white_to_move:?}"
    );
    assert!(
        black_to_move.win_chance() < 0.2,
        "the side a queen down should be losing: {black_to_move:?}"
    );
    assert!(
        white_to_move.win_chance() > black_to_move.win_chance(),
        "the same position must not favour whoever happens to be moving"
    );
}

#[test]
fn strength_can_be_capped_and_released() {
    let mut engine = engine_or_skip!();
    // Below the engine's own floor; the wrapper raises it rather than failing.
    engine
        .limit_strength(Some(MIN_LIMITED_ELO - 500))
        .expect("capping strength should be accepted");
    let capped = engine
        .analyse(START_FEN, &[], Limit::Movetime(Duration::from_millis(200)))
        .expect("a capped engine still plays");
    assert!(capped.best_move.is_some());

    engine.limit_strength(None).expect("cap should lift");
    let full = engine
        .analyse(START_FEN, &[], Limit::Depth(10))
        .expect("full strength analysis");
    assert!(full.best_move.is_some());
}

#[test]
fn a_position_with_no_legal_moves_returns_no_move() {
    let mut engine = engine_or_skip!();
    // Back-rank mate: the rook is out of the king's reach and every escape
    // square is either covered or blocked by Black's own pawns.
    let analysis = engine
        .analyse(
            "4R1k1/5ppp/8/8/8/8/5PPP/6K1 b - - 0 1",
            &[],
            Limit::Depth(4),
        )
        .expect("analysis should succeed");
    assert_eq!(
        analysis.best_move, None,
        "expected no move in a mated position"
    );
}

/// The whole point of the play mode: a real engine reviewing a real game, and
/// the mistakes it finds turning into puzzles the trainer can serve.
#[test]
fn a_blundered_game_becomes_training_material() {
    use omachess_core::puzzle::Attempt;
    use omachess_core::review::{analyse_game, puzzle_from, Severity};
    use shakmaty::Color;

    let mut engine = engine_or_skip!();

    // 1. e4 e5 2. Nf3 Nc6 3. Bc4 Qg5?? — Black hangs the queen to Nxg5.
    let moves: Vec<String> = ["e2e4", "e7e5", "g1f3", "b8c6", "f1c4", "d8g5"]
        .iter()
        .map(|m| m.to_string())
        .collect();

    let analysis =
        analyse_game(&mut engine, START_FEN, &moves, Color::Black).expect("review should succeed");

    let flagged = analysis.drillable();
    let blunder = flagged
        .iter()
        .find(|r| r.played == "d8g5")
        .unwrap_or_else(|| panic!("hanging the queen was not flagged: {flagged:?}"));
    assert_eq!(blunder.severity, Some(Severity::Blunder));
    assert!(
        blunder.lost() > 0.3,
        "a hung queen should cost a lot of win probability: {}",
        blunder.lost()
    );

    // And the puzzle it produces must actually be solvable by the trainer.
    let puzzle = puzzle_from(blunder, 1200, "own-test");
    let attempt = Attempt::new(&puzzle).expect("generated puzzle should load");
    assert_eq!(attempt.expected(), Some(blunder.best.as_str()));
    assert!(puzzle.themes.contains(&"fromMyGame".to_string()));
}

/// Sound play should not be flagged, or the deck fills with noise.
#[test]
fn a_reasonable_opening_produces_no_blunders() {
    use omachess_core::review::analyse_game;
    use shakmaty::Color;

    let mut engine = engine_or_skip!();
    // 1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 — the Ruy Lopez, Morphy Defence.
    let moves: Vec<String> = ["e2e4", "e7e5", "g1f3", "b8c6", "f1b5", "a7a6"]
        .iter()
        .map(|m| m.to_string())
        .collect();

    let analysis =
        analyse_game(&mut engine, START_FEN, &moves, Color::Black).expect("review should succeed");
    assert!(
        analysis.counts().blunders == 0,
        "book moves were called blunders: {:?}",
        analysis.drillable()
    );
    // And a sound opening should score well on the per-move measure too.
    assert!(
        analysis.accuracy() > 80.0,
        "book moves scored only {:.1}% accuracy",
        analysis.accuracy()
    );
}
