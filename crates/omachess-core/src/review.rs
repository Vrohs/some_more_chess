//! Turning a played game into training material.
//!
//! This is what makes playing part of studying rather than a diversion beside
//! it: every position the player got wrong becomes a puzzle, entering the same
//! spaced-repetition and fluency loop as everything else. The mistakes a player
//! actually makes are better training material than any curated set, because
//! they are drawn from that player's own blind spots.

use anyhow::{anyhow, Context, Result};
use shakmaty::fen::Fen;
use shakmaty::uci::UciMove;
use shakmaty::{CastlingMode, Chess, Color, EnPassantMode, Position};

use crate::engine::{Analysis, Engine, Limit};
use crate::puzzle::Puzzle;

/// Win-probability drops that Lichess treats as each class of error.
pub const INACCURACY: f64 = 0.10;
pub const MISTAKE: f64 = 0.20;
pub const BLUNDER: f64 = 0.30;

/// Positions at or beyond this win probability are already decided, so a drop
/// there is not worth drilling — you cannot learn much from throwing away one
/// of several winning continuations.
pub const DECIDED: f64 = 0.97;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Inaccuracy,
    Mistake,
    Blunder,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Inaccuracy => "inaccuracy",
            Severity::Mistake => "mistake",
            Severity::Blunder => "blunder",
        }
    }
}

/// How badly one move went, judged in win probability rather than material.
pub fn classify(win_before: f64, win_after: f64) -> Option<Severity> {
    if win_before >= DECIDED && win_after >= DECIDED {
        return None;
    }
    let lost = win_before - win_after;
    if lost >= BLUNDER {
        Some(Severity::Blunder)
    } else if lost >= MISTAKE {
        Some(Severity::Mistake)
    } else if lost >= INACCURACY {
        Some(Severity::Inaccuracy)
    } else {
        None
    }
}

/// Where in the game a move was played. Knowing that your endgame leaks twice
/// as much as your middlegame is actionable in a way that a result is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    Opening,
    Middlegame,
    Endgame,
}

impl Phase {
    /// The theme a puzzle from this phase carries, matching the names the
    /// puzzle corpus already uses so both sets can be selected the same way.
    pub fn theme(self) -> &'static str {
        match self {
            Phase::Opening => "opening",
            Phase::Middlegame => "middlegame",
            Phase::Endgame => "endgame",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Phase::Opening => "opening",
            Phase::Middlegame => "middlegame",
            Phase::Endgame => "endgame",
        }
    }
}

/// Full plies still counted as the opening.
const OPENING_PLIES: usize = 20;
/// Combined non-pawn material at or below which the position is an endgame.
/// A queen and rook for one side is fourteen, so this sits just under it.
const ENDGAME_MATERIAL: u32 = 13;

fn phase_of(position: &Chess, ply: usize) -> Phase {
    let board = position.board();
    let mut material = 0;
    for index in 0..64u32 {
        let square = shakmaty::Square::new(index);
        if let Some(piece) = board.piece_at(square) {
            material += match piece.role {
                shakmaty::Role::Queen => 9,
                shakmaty::Role::Rook => 5,
                shakmaty::Role::Bishop | shakmaty::Role::Knight => 3,
                _ => 0,
            };
        }
    }
    if material <= ENDGAME_MATERIAL {
        Phase::Endgame
    } else if ply < OPENING_PLIES {
        Phase::Opening
    } else {
        Phase::Middlegame
    }
}

/// One of the player's moves, judged against what the engine preferred.
#[derive(Debug, Clone, PartialEq)]
pub struct MoveAnalysis {
    pub ply: usize,
    pub setup_fen: String,
    pub setup_move: String,
    pub played: String,
    pub best: String,
    /// The engine's continuation after its preferred move — what you would
    /// have got, which is the part that actually teaches.
    pub best_line: Vec<String>,
    pub phase: Phase,
    /// Win probability before and after, both from the mover's point of view.
    pub win_before: f64,
    pub win_after: f64,
    pub severity: Option<Severity>,
}

impl MoveAnalysis {
    /// Win probability given away, never negative — finding a move better than
    /// the engine's preferred one is not a credit to be banked against blunders.
    pub fn lost(&self) -> f64 {
        (self.win_before - self.win_after).max(0.0)
    }

    /// Accuracy of this single move, on the scale Lichess uses.
    pub fn accuracy(&self) -> f64 {
        let lost_percent = self.lost() * 100.0;
        (103.166_8 * (-0.043_54 * lost_percent).exp() - 3.166_9).clamp(0.0, 100.0)
    }

    /// Whether this move can be turned into a puzzle.
    pub fn is_drillable(&self) -> bool {
        self.severity.is_some() && self.best != self.played && !self.setup_move.is_empty()
    }
}

/// How many of each kind of error a game contained.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub inaccuracies: usize,
    pub mistakes: usize,
    pub blunders: usize,
}

/// Everything measurable about how one side played one game.
///
/// Deliberately says nothing about who won. A win against a weak opponent and a
/// loss against a strong one say little about whether you played well; the win
/// probability you gave away per move says a great deal.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GameAnalysis {
    pub moves: Vec<MoveAnalysis>,
}

impl GameAnalysis {
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }

    /// Mean per-move accuracy, 0–100.
    pub fn accuracy(&self) -> f64 {
        if self.moves.is_empty() {
            return 0.0;
        }
        self.moves.iter().map(MoveAnalysis::accuracy).sum::<f64>() / self.moves.len() as f64
    }

    /// Mean win probability given away per move.
    pub fn mean_loss(&self) -> f64 {
        if self.moves.is_empty() {
            return 0.0;
        }
        self.moves.iter().map(MoveAnalysis::lost).sum::<f64>() / self.moves.len() as f64
    }

    /// Median loss, which a single catastrophe cannot move.
    pub fn median_loss(&self) -> f64 {
        if self.moves.is_empty() {
            return 0.0;
        }
        let mut losses: Vec<f64> = self.moves.iter().map(MoveAnalysis::lost).collect();
        losses.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = losses.len() / 2;
        if losses.len().is_multiple_of(2) {
            (losses[mid - 1] + losses[mid]) / 2.0
        } else {
            losses[mid]
        }
    }

    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for m in &self.moves {
            match m.severity {
                Some(Severity::Inaccuracy) => counts.inaccuracies += 1,
                Some(Severity::Mistake) => counts.mistakes += 1,
                Some(Severity::Blunder) => counts.blunders += 1,
                None => {}
            }
        }
        counts
    }

    /// Mean loss within each phase that actually occurred, worst first — the
    /// most directly actionable thing a game report can say.
    pub fn by_phase(&self) -> Vec<(Phase, f64, usize)> {
        let mut out = Vec::new();
        for phase in [Phase::Opening, Phase::Middlegame, Phase::Endgame] {
            let subset: Vec<&MoveAnalysis> =
                self.moves.iter().filter(|m| m.phase == phase).collect();
            if subset.is_empty() {
                continue;
            }
            let mean = subset.iter().map(|m| m.lost()).sum::<f64>() / subset.len() as f64;
            out.push((phase, mean, subset.len()));
        }
        out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    /// The single move where the most was given away — the moment the game
    /// turned. Worth practising above every other position in the game.
    pub fn critical_moment(&self) -> Option<&MoveAnalysis> {
        self.moves
            .iter()
            .filter(|m| m.is_drillable())
            .max_by(|a, b| {
                a.lost()
                    .partial_cmp(&b.lost())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    /// The moves worth drilling, worst first.
    pub fn drillable(&self) -> Vec<&MoveAnalysis> {
        let mut out: Vec<&MoveAnalysis> = self.moves.iter().filter(|m| m.is_drillable()).collect();
        out.sort_by(|a, b| {
            b.lost()
                .partial_cmp(&a.lost())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out
    }
}

/// Anything that can evaluate a position, so a review can be tested without a
/// real engine process.
pub trait Evaluator {
    fn eval(&mut self, fen: &str, moves: &[String]) -> Result<Analysis>;
}

/// An engine held to a fixed search depth.
///
/// Bulk import analyses thousands of positions, where the default depth would
/// take hours. A shallower search is less certain about close calls but finds
/// the same blunders, which is what the import is for.
pub struct AtDepth<'a> {
    pub engine: &'a mut Engine,
    pub depth: u32,
}

impl Evaluator for AtDepth<'_> {
    fn eval(&mut self, fen: &str, moves: &[String]) -> Result<Analysis> {
        self.engine.analyse(fen, moves, Limit::Depth(self.depth))
    }
}

impl Evaluator for Engine {
    fn eval(&mut self, fen: &str, moves: &[String]) -> Result<Analysis> {
        self.analyse(fen, moves, Limit::Depth(16))
    }
}

/// Analyse every move `player` made, whether or not it was a mistake.
///
/// Recording only the errors would make an average impossible: a game with two
/// blunders and forty precise moves would look identical to one with two
/// blunders and nothing else.
pub fn analyse_game(
    evaluator: &mut impl Evaluator,
    initial_fen: &str,
    moves: &[String],
    player: Color,
) -> Result<GameAnalysis> {
    let setup: Fen = initial_fen.parse().context("invalid starting FEN")?;
    let mut position: Chess = setup
        .into_position(CastlingMode::Standard)
        .map_err(|e| anyhow!("illegal starting position: {e}"))?;

    let mut analysis = GameAnalysis::default();
    // The opponent move that produced the position now on the board.
    let mut previous: Option<(String, String)> = None;

    for (ply, played) in moves.iter().enumerate() {
        let fen_before = fen_of(&position);
        let mover = position.turn();

        let uci: UciMove = played
            .parse()
            .with_context(|| format!("bad move {played}"))?;
        let mv = uci
            .to_move(&position)
            .map_err(|e| anyhow!("move {played} is not legal at ply {ply}: {e}"))?;

        if mover == player {
            let before = evaluator.eval(&fen_before, &[])?;
            let after = evaluator.eval(&fen_before, std::slice::from_ref(played))?;

            if let (Some(before_score), Some(after_score)) = (before.score, after.score) {
                let win_before = before_score.win_chance();
                // The evaluation after the move is from the opponent's side.
                let win_after = 1.0 - after_score.win_chance();
                let best = before.best_move.clone().unwrap_or_else(|| played.clone());

                // A move that matches the engine's own choice cannot have lost
                // anything, whatever a shallow search says about the two
                // positions.
                let matched_engine = best == *played;
                let severity = if matched_engine {
                    None
                } else {
                    classify(win_before, win_after)
                };
                let (setup_fen, setup_move) = previous.clone().unwrap_or_default();

                analysis.moves.push(MoveAnalysis {
                    ply,
                    setup_fen,
                    setup_move,
                    played: played.clone(),
                    best,
                    best_line: before.pv.clone(),
                    phase: phase_of(&position, ply),
                    win_before,
                    win_after: if matched_engine {
                        win_before
                    } else {
                        win_after
                    },
                    severity,
                });
            }
        }

        position.play_unchecked(mv);
        previous = Some((fen_before, played.clone()));
    }
    Ok(analysis)
}

/// Depth the answer is re-checked at before it is allowed to become a puzzle.
/// Deeper than the review pass, because a puzzle asserts its answer as correct
/// and a wrong puzzle teaches a wrong pattern.
pub const CONFIRM_DEPTH: u32 = 20;

/// Re-examine each move worth drilling and drop the ones a deeper search
/// disagrees with.
///
/// The review pass is a compromise between accuracy and time across a whole
/// game. That is fine for a report — a mistaken severity costs a line of text.
/// It is not fine for a puzzle, which presents its answer as the truth, so
/// every candidate is confirmed before it can become one.
///
/// Returns how many were rejected.
pub fn confirm_drillable(
    evaluator: &mut impl Evaluator,
    analysis: &mut GameAnalysis,
) -> Result<usize> {
    let mut rejected = 0;
    for review in &mut analysis.moves {
        if !review.is_drillable() {
            continue;
        }
        let deeper = evaluator.eval(&review.setup_fen, std::slice::from_ref(&review.setup_move))?;
        let confirmed = deeper
            .best_move
            .as_deref()
            .is_some_and(|best| best == review.best);
        if !confirmed {
            // Not certain enough to teach, so it stays in the report as a
            // note and stops being an exercise.
            review.severity = None;
            rejected += 1;
        } else if !deeper.pv.is_empty() {
            // Keep the better line while it is in hand.
            review.best_line = deeper.pv.clone();
        }
    }
    Ok(rejected)
}

/// Build a puzzle from a reviewed mistake.
///
/// The shape matches the Lichess export exactly — a position, the opponent move
/// that created it, then the move to find — so generated puzzles flow through
/// the same solving, scheduling and fluency code as downloaded ones.
/// The theme marking a puzzle taken from the player's own game.
pub const OWN_GAME_THEME: &str = "fromMyGame";

pub fn puzzle_from(review: &MoveAnalysis, rating: u32, id: &str) -> Puzzle {
    Puzzle {
        id: id.to_owned(),
        fen: review.setup_fen.clone(),
        moves: vec![review.setup_move.clone(), review.best.clone()],
        rating,
        rating_deviation: 0,
        popularity: 0,
        nb_plays: 0,
        themes: vec![
            OWN_GAME_THEME.into(),
            // The phase the mistake was made in, so a player whose losses come
            // from one part of the game can train only that part.
            review.phase.theme().into(),
            review
                .severity
                .map(Severity::label)
                .unwrap_or("inaccuracy")
                .into(),
        ],
        game_url: String::new(),
        opening_tags: Vec::new(),
    }
}

/// A stable identifier for a position the player got wrong.
///
/// Derived from the position and the answer rather than from the clock, so
/// blundering the same way twice updates one puzzle instead of accumulating
/// near-duplicates in the deck.
pub fn stable_puzzle_id(review: &MoveAnalysis) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in review
        .setup_fen
        .as_bytes()
        .iter()
        .chain(review.setup_move.as_bytes())
        .chain(review.best.as_bytes())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("own{hash:012x}")
}

fn fen_of(position: &Chess) -> String {
    Fen::from_position(position, EnPassantMode::Legal).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Score;

    #[test]
    fn severity_follows_how_much_was_given_away() {
        assert_eq!(classify(0.55, 0.50), None);
        assert_eq!(classify(0.55, 0.42), Some(Severity::Inaccuracy));
        assert_eq!(classify(0.55, 0.30), Some(Severity::Mistake));
        assert_eq!(classify(0.55, 0.10), Some(Severity::Blunder));
    }

    #[test]
    fn an_already_won_position_is_not_drilled() {
        // Both sides of the move are still overwhelming; nothing to learn.
        assert_eq!(classify(0.999, 0.975), None);
    }

    #[test]
    fn throwing_away_a_won_game_still_counts() {
        assert_eq!(classify(0.99, 0.40), Some(Severity::Blunder));
    }

    /// Returns a fixed evaluation for every position, except the one where the
    /// player is set up to blunder.
    struct ScriptedEvaluator {
        blunder_after: String,
    }

    impl Evaluator for ScriptedEvaluator {
        fn eval(&mut self, _fen: &str, moves: &[String]) -> Result<Analysis> {
            let played = moves.first().cloned().unwrap_or_default();
            if played == self.blunder_after {
                // Seen from the opponent, who is now winning.
                return Ok(Analysis {
                    best_move: Some("e7e5".into()),
                    score: Some(Score::Cp(900)),
                    depth: 16,
                    pv: vec![],
                });
            }
            Ok(Analysis {
                best_move: Some("d2d4".into()),
                score: Some(Score::Cp(0)),
                depth: 16,
                pv: vec![],
            })
        }
    }

    #[test]
    fn a_blunder_is_found_and_becomes_a_solvable_puzzle() {
        let mut evaluator = ScriptedEvaluator {
            blunder_after: "g1h3".into(),
        };
        // 1. d4 d5 2. Nh3?? — the third player move is the bad one.
        let moves: Vec<String> = ["d2d4", "d7d5", "g1h3"]
            .iter()
            .map(|m| m.to_string())
            .collect();
        let analysis = analyse_game(
            &mut evaluator,
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
            &moves,
            Color::White,
        )
        .unwrap();

        let flagged = analysis.drillable();
        assert_eq!(flagged.len(), 1, "expected one flagged move: {flagged:?}");
        let review = flagged[0];
        assert_eq!(review.played, "g1h3");
        assert_eq!(review.severity, Some(Severity::Blunder));
        assert_eq!(review.setup_move, "d7d5");

        // The generated puzzle must be playable by the existing solver.
        let puzzle = puzzle_from(review, 1200, "own-1");
        let attempt = crate::puzzle::Attempt::new(&puzzle).unwrap();
        assert_eq!(attempt.expected(), Some(review.best.as_str()));
        assert!(puzzle.themes.contains(&"fromMyGame".to_string()));
    }

    #[test]
    fn a_generated_id_is_stable_and_distinct() {
        let a = MoveAnalysis {
            ply: 4,
            setup_fen: "8/8/8/8/8/8/8/K6k w - - 0 1".into(),
            setup_move: "a1a2".into(),
            played: "h1h2".into(),
            best: "h1g2".into(),
            best_line: Vec::new(),
            phase: Phase::Endgame,
            win_before: 0.5,
            win_after: 0.2,
            severity: Some(Severity::Mistake),
        };
        assert_eq!(stable_puzzle_id(&a), stable_puzzle_id(&a), "must be stable");

        // A different answer in the same position is a different exercise.
        let mut b = a.clone();
        b.best = "h1h2".into();
        assert_ne!(stable_puzzle_id(&a), stable_puzzle_id(&b));

        // The ply it happened on must not change the identity.
        let mut c = a.clone();
        c.ply = 40;
        assert_eq!(stable_puzzle_id(&a), stable_puzzle_id(&c));
    }

    /// Answers whatever it is told to, so confirmation can be exercised without
    /// an engine.
    struct FixedEvaluator {
        best: &'static str,
        pv: Vec<String>,
    }

    impl Evaluator for FixedEvaluator {
        fn eval(&mut self, _fen: &str, _moves: &[String]) -> Result<Analysis> {
            Ok(Analysis {
                best_move: Some(self.best.to_owned()),
                score: Some(Score::Cp(0)),
                depth: CONFIRM_DEPTH,
                pv: self.pv.clone(),
            })
        }
    }

    fn drillable_move() -> MoveAnalysis {
        MoveAnalysis {
            ply: 3,
            setup_fen: "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1".into(),
            setup_move: "e2e4".into(),
            played: "b8c6".into(),
            best: "e7e5".into(),
            best_line: vec!["e7e5".into()],
            phase: Phase::Opening,
            win_before: 0.5,
            win_after: 0.2,
            severity: Some(Severity::Mistake),
        }
    }

    #[test]
    fn an_answer_a_deeper_search_agrees_with_survives() {
        let mut analysis = GameAnalysis {
            moves: vec![drillable_move()],
        };
        let mut evaluator = FixedEvaluator {
            best: "e7e5",
            pv: vec!["e7e5".into(), "g1f3".into()],
        };
        assert_eq!(confirm_drillable(&mut evaluator, &mut analysis).unwrap(), 0);
        assert_eq!(analysis.drillable().len(), 1);
        assert_eq!(
            analysis.moves[0].best_line,
            vec!["e7e5", "g1f3"],
            "the deeper line should replace the shallow one"
        );
    }

    #[test]
    fn an_answer_a_deeper_search_disagrees_with_never_becomes_a_puzzle() {
        let mut analysis = GameAnalysis {
            moves: vec![drillable_move()],
        };
        let mut evaluator = FixedEvaluator {
            best: "c7c5",
            pv: Vec::new(),
        };
        assert_eq!(confirm_drillable(&mut evaluator, &mut analysis).unwrap(), 1);
        assert!(
            analysis.drillable().is_empty(),
            "an unconfirmed answer must not be taught"
        );
    }

    #[test]
    fn confirmation_leaves_sound_moves_alone() {
        let quiet = MoveAnalysis {
            severity: None,
            ..drillable_move()
        };
        let mut analysis = GameAnalysis { moves: vec![quiet] };
        let mut evaluator = FixedEvaluator {
            best: "anything",
            pv: Vec::new(),
        };
        assert_eq!(confirm_drillable(&mut evaluator, &mut analysis).unwrap(), 0);
    }

    #[test]
    fn accuracy_is_perfect_when_nothing_is_given_away() {
        let mut analysis = GameAnalysis::default();
        analysis.moves.push(MoveAnalysis {
            ply: 0,
            setup_fen: String::new(),
            setup_move: String::new(),
            played: "e2e4".into(),
            best: "e2e4".into(),
            best_line: Vec::new(),
            phase: Phase::Opening,
            win_before: 0.5,
            win_after: 0.5,
            severity: None,
        });
        assert!(analysis.accuracy() > 99.0, "got {}", analysis.accuracy());
        assert!(analysis.mean_loss().abs() < 1e-9);
    }

    #[test]
    fn a_single_blunder_moves_the_mean_but_not_the_median() {
        let quiet = |ply| MoveAnalysis {
            ply,
            setup_fen: String::new(),
            setup_move: String::new(),
            played: "a2a3".into(),
            best: "a2a3".into(),
            best_line: Vec::new(),
            phase: Phase::Middlegame,
            win_before: 0.5,
            win_after: 0.5,
            severity: None,
        };
        let mut analysis = GameAnalysis {
            moves: (0..9).map(quiet).collect(),
        };
        analysis.moves.push(MoveAnalysis {
            ply: 9,
            setup_fen: "fen".into(),
            setup_move: "e7e5".into(),
            played: "d1h5".into(),
            best: "g1f3".into(),
            best_line: Vec::new(),
            phase: Phase::Middlegame,
            win_before: 0.9,
            win_after: 0.1,
            severity: Some(Severity::Blunder),
        });
        assert!(analysis.mean_loss() > 0.0, "the mean must feel the blunder");
        assert!(
            analysis.median_loss().abs() < 1e-9,
            "the median must not, got {}",
            analysis.median_loss()
        );
        assert_eq!(analysis.counts().blunders, 1);
        assert_eq!(analysis.drillable().len(), 1);
    }

    #[test]
    fn finding_better_than_the_engine_is_not_banked_as_credit() {
        let m = MoveAnalysis {
            ply: 0,
            setup_fen: String::new(),
            setup_move: String::new(),
            played: "e2e4".into(),
            best: "d2d4".into(),
            best_line: Vec::new(),
            phase: Phase::Opening,
            win_before: 0.4,
            win_after: 0.6,
            severity: None,
        };
        assert_eq!(m.lost(), 0.0, "a gain must not offset a later loss");
    }

    #[test]
    fn the_opponents_moves_are_not_reviewed() {
        let mut evaluator = ScriptedEvaluator {
            blunder_after: "d7d5".into(),
        };
        let moves: Vec<String> = ["d2d4", "d7d5"].iter().map(|m| m.to_string()).collect();
        let analysis = analyse_game(
            &mut evaluator,
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
            &moves,
            Color::White,
        )
        .unwrap();
        // Every move now gets a record, so the property to check is that only
        // the player's own moves appear — White's, at the even plies.
        assert_eq!(analysis.moves.len(), 1, "got {:?}", analysis.moves);
        assert_eq!(analysis.moves[0].played, "d2d4");
        assert!(
            analysis.moves.iter().all(|m| m.ply.is_multiple_of(2)),
            "an opponent move was analysed: {:?}",
            analysis.moves
        );
    }
}

/// Whether errors cluster on the moves that were played quickly.
///
/// This is the one question a chess engine cannot answer on its own: it needs
/// how long each move took, which only the application knows. If the blunders
/// are concentrated in the fast moves, the fix is a habit rather than more
/// tactics — and that is a different piece of advice entirely.
#[derive(Debug, Clone, PartialEq)]
pub struct TimePressure {
    /// Moves that had both an analysis and a recorded time.
    pub moves: usize,
    pub median_time: std::time::Duration,
    pub quick_moves: usize,
    pub considered_moves: usize,
    /// Mean win probability given away, split at the median think time.
    pub quick_loss: f64,
    pub considered_loss: f64,
    /// Median time spent on moves that were errors, and on those that were not.
    pub error_time: Option<std::time::Duration>,
    pub clean_time: Option<std::time::Duration>,
    /// One-sided probability that quick moves are not worse.
    pub p_value: f64,
}

impl TimePressure {
    pub fn is_significant(&self) -> bool {
        self.p_value <= crate::progress::SIGNIFICANT
    }
}

/// Moves needed before the split is worth reporting.
pub const MIN_TIMED_MOVES: usize = 8;

/// Pair each analysed move with how long it took and compare the halves.
///
/// `times` holds one entry per move the player made, in order. A move's index
/// is derived from its ply rather than its position in the analysis, so a move
/// the engine failed to score cannot shift every later pairing.
pub fn time_pressure(
    moves: &[MoveAnalysis],
    times: &[std::time::Duration],
) -> Option<TimePressure> {
    let paired: Vec<(&MoveAnalysis, std::time::Duration)> = moves
        .iter()
        .filter_map(|m| times.get(m.ply / 2).map(|t| (m, *t)))
        .collect();

    if paired.len() < MIN_TIMED_MOVES {
        return None;
    }

    let mut sorted: Vec<std::time::Duration> = paired.iter().map(|(_, t)| *t).collect();
    sorted.sort();
    let median = sorted[sorted.len() / 2];

    let (quick, considered): (Vec<_>, Vec<_>) = paired.iter().partition(|(_, t)| *t < median);
    if quick.is_empty() || considered.is_empty() {
        return None;
    }

    let mean = |set: &[&(&MoveAnalysis, std::time::Duration)]| -> f64 {
        set.iter().map(|(m, _)| m.lost()).sum::<f64>() / set.len() as f64
    };
    let losses = |set: &[&(&MoveAnalysis, std::time::Duration)]| -> Vec<f64> {
        set.iter().map(|(m, _)| m.lost()).collect()
    };
    let median_time_of = |set: &[std::time::Duration]| -> Option<std::time::Duration> {
        if set.is_empty() {
            return None;
        }
        let mut v = set.to_vec();
        v.sort();
        Some(v[v.len() / 2])
    };

    let quick_refs: Vec<_> = quick.iter().collect();
    let considered_refs: Vec<_> = considered.iter().collect();

    let error_times: Vec<std::time::Duration> = paired
        .iter()
        .filter(|(m, _)| m.severity.is_some())
        .map(|(_, t)| *t)
        .collect();
    let clean_times: Vec<std::time::Duration> = paired
        .iter()
        .filter(|(m, _)| m.severity.is_none())
        .map(|(_, t)| *t)
        .collect();

    Some(TimePressure {
        moves: paired.len(),
        median_time: median,
        quick_moves: quick.len(),
        considered_moves: considered.len(),
        quick_loss: mean(&quick_refs),
        considered_loss: mean(&considered_refs),
        error_time: median_time_of(&error_times),
        clean_time: median_time_of(&clean_times),
        p_value: crate::progress::mann_whitney_greater(
            &losses(&quick_refs),
            &losses(&considered_refs),
        ),
    })
}

#[cfg(test)]
mod time_tests {
    use super::*;
    use std::time::Duration;

    fn move_at(ply: usize, lost: f64) -> MoveAnalysis {
        MoveAnalysis {
            ply,
            setup_fen: String::new(),
            setup_move: String::new(),
            played: "a2a3".into(),
            best: if lost > 0.0 {
                "b2b3".into()
            } else {
                "a2a3".into()
            },
            best_line: Vec::new(),
            phase: Phase::Middlegame,
            win_before: 0.5,
            win_after: 0.5 - lost,
            severity: (lost >= INACCURACY).then_some(Severity::Blunder),
        }
    }

    #[test]
    fn nothing_is_claimed_from_too_few_moves() {
        let moves: Vec<_> = (0..4).map(|i| move_at(i * 2, 0.0)).collect();
        let times = vec![Duration::from_secs(5); 4];
        assert!(time_pressure(&moves, &times).is_none());
    }

    #[test]
    fn errors_concentrated_in_quick_moves_are_detected() {
        // Ten moves: the five quick ones are all bad, the five slow ones clean.
        let mut moves = Vec::new();
        let mut times = Vec::new();
        for i in 0..5 {
            moves.push(move_at(i * 2, 0.35));
            times.push(Duration::from_secs(2));
        }
        for i in 5..10 {
            moves.push(move_at(i * 2, 0.0));
            times.push(Duration::from_secs(30));
        }
        let result = time_pressure(&moves, &times).expect("a result");
        assert!(result.quick_loss > result.considered_loss);
        assert!(result.is_significant(), "p was {}", result.p_value);
        assert!(result.error_time.unwrap() < result.clean_time.unwrap());
    }

    #[test]
    fn errors_spread_evenly_are_not_blamed_on_speed() {
        let mut moves = Vec::new();
        let mut times = Vec::new();
        for i in 0..12 {
            moves.push(move_at(i * 2, if i % 2 == 0 { 0.3 } else { 0.0 }));
            times.push(Duration::from_secs(if i < 6 { 2 } else { 30 }));
        }
        let result = time_pressure(&moves, &times).expect("a result");
        assert!(
            !result.is_significant(),
            "evenly spread errors blamed on speed, p = {}",
            result.p_value
        );
    }

    #[test]
    fn a_move_the_engine_could_not_score_does_not_shift_the_pairing() {
        // Ply 6 is missing from the analysis; later moves must still line up
        // with their own times rather than sliding one place.
        let moves = vec![
            move_at(0, 0.0),
            move_at(2, 0.0),
            move_at(4, 0.0),
            move_at(8, 0.4),
            move_at(10, 0.0),
            move_at(12, 0.0),
            move_at(14, 0.0),
            move_at(16, 0.0),
        ];
        let mut times = vec![Duration::from_secs(20); 9];
        times[4] = Duration::from_secs(1); // the blunder at ply 8
        let result = time_pressure(&moves, &times).expect("a result");
        assert_eq!(result.moves, 8);
        assert!(
            result.error_time == Some(Duration::from_secs(1)),
            "the blunder was paired with the wrong time: {:?}",
            result.error_time
        );
    }
}
