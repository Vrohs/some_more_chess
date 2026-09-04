//! Replaying the position where a game turned, as an exercise.
//!
//! This lived in the GTK view, which meant none of it could be tested: whether
//! the right move is recognised, when the answer should be given up, and which
//! line gets played back are all decisions, not drawing.

use shakmaty::uci::UciMove;
use shakmaty::{CastlingMode, Chess, Position, Square};

use crate::game::find_move;
use crate::review::MoveAnalysis;

/// Wrong answers allowed before the line is shown. The attempt already counts
/// as a failure in the game it came from, so withholding it past this point
/// teaches nothing.
pub const REVEAL_AFTER_MISSES: u32 = 2;

/// What offering a move to the exercise did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offer {
    /// The move the engine preferred.
    Correct,
    /// A legal move, but not the one being looked for.
    Wrong { reveal: bool },
    /// Not a legal move at all; nothing happened.
    Illegal,
}

pub struct Drill {
    position: Chess,
    expected: String,
    /// The engine's continuation from the position, as it reported it.
    continuation: Vec<String>,
    misses: u32,
    solved: bool,
}

impl Drill {
    /// Build an exercise from a reviewed move, or `None` if the position it
    /// came from cannot be reconstructed.
    pub fn from_analysis(analysis: &MoveAnalysis) -> Option<Self> {
        let position = position_after(&analysis.setup_fen, &analysis.setup_move)?;
        // A move that matches what was played is not an exercise.
        if analysis.best == analysis.played {
            return None;
        }
        // The answer must be legal in the position being posed.
        let uci: UciMove = analysis.best.parse().ok()?;
        uci.to_move(&position).ok()?;

        Some(Self {
            position,
            expected: analysis.best.clone(),
            continuation: analysis.best_line.clone(),
            misses: 0,
            solved: false,
        })
    }

    pub fn position(&self) -> &Chess {
        &self.position
    }

    pub fn expected(&self) -> &str {
        &self.expected
    }

    pub fn misses(&self) -> u32 {
        self.misses
    }

    pub fn is_solved(&self) -> bool {
        self.solved
    }

    /// Offer a move by the squares it was dragged between.
    pub fn offer(&mut self, from: Square, to: Square) -> Offer {
        if self.solved {
            return Offer::Illegal;
        }
        let Some(mv) = find_move(&self.position, from, to, Some(&self.expected)) else {
            return Offer::Illegal;
        };
        if UciMove::from_standard(mv).to_string() == self.expected {
            self.solved = true;
            return Offer::Correct;
        }
        self.misses += 1;
        let reveal = self.misses >= REVEAL_AFTER_MISSES;
        if reveal {
            self.solved = true;
        }
        Offer::Wrong { reveal }
    }

    /// The moves to play out when showing the answer: the move that should have
    /// been played, then the engine's continuation.
    ///
    /// Engines report their principal variation starting with their own choice,
    /// so that first move is dropped rather than played twice.
    pub fn reveal_line(&self) -> Vec<String> {
        let mut line = vec![self.expected.clone()];
        line.extend(
            self.continuation
                .iter()
                .skip_while(|m| m.as_str() == self.expected)
                .cloned(),
        );
        line
    }
}

/// The position a player faced: a FEN with the opponent's move applied.
pub fn position_after(fen: &str, mv: &str) -> Option<Chess> {
    let mut position = fen
        .parse::<shakmaty::fen::Fen>()
        .ok()?
        .into_position::<Chess>(CastlingMode::Standard)
        .ok()?;
    let parsed: UciMove = mv.parse().ok()?;
    let played = parsed.to_move(&position).ok()?;
    position.play_unchecked(played);
    Some(position)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::{MoveAnalysis, Phase, Severity};

    /// After 1.e4 e5 2.Bc4 Nc6 3.Qh5, Black played Nf6?? losing to Qxf7#.
    /// The move that should have been played is g7g6.
    fn blunder() -> MoveAnalysis {
        MoveAnalysis {
            ply: 5,
            setup_fen: "r1bqkbnr/pppp1ppp/2n5/4p3/2B1P3/8/PPPP1PPP/RNBQK1NR w KQkq - 0 3".into(),
            setup_move: "d1h5".into(),
            played: "g8f6".into(),
            best: "g7g6".into(),
            best_line: vec!["g7g6".into(), "h5f3".into(), "g8f6".into()],
            phase: Phase::Opening,
            win_before: 0.5,
            win_after: 0.02,
            severity: Some(Severity::Blunder),
        }
    }

    #[test]
    fn the_exercise_poses_the_position_the_player_faced() {
        let drill = Drill::from_analysis(&blunder()).expect("a drill");
        assert_eq!(drill.expected(), "g7g6");
        assert_eq!(
            drill.position().turn(),
            shakmaty::Color::Black,
            "the exercise must be posed to the side that erred"
        );
        assert!(!drill.is_solved());
    }

    #[test]
    fn playing_the_right_move_solves_it_first_time() {
        let mut drill = Drill::from_analysis(&blunder()).unwrap();
        assert_eq!(drill.offer(Square::G7, Square::G6), Offer::Correct);
        assert!(drill.is_solved());
        assert_eq!(drill.misses(), 0);
    }

    #[test]
    fn the_answer_is_given_up_after_two_wrong_moves() {
        let mut drill = Drill::from_analysis(&blunder()).unwrap();
        assert_eq!(
            drill.offer(Square::G8, Square::F6),
            Offer::Wrong { reveal: false },
            "one wrong move should not give it away"
        );
        assert!(!drill.is_solved());
        assert_eq!(
            drill.offer(Square::D7, Square::D6),
            Offer::Wrong { reveal: true },
            "the second wrong move should"
        );
        assert!(drill.is_solved());
        assert_eq!(drill.misses(), 2);
    }

    #[test]
    fn an_illegal_drag_is_not_counted_as_a_wrong_answer() {
        let mut drill = Drill::from_analysis(&blunder()).unwrap();
        assert_eq!(drill.offer(Square::A1, Square::H8), Offer::Illegal);
        assert_eq!(drill.misses(), 0, "a misdrag must not use up an attempt");
    }

    #[test]
    fn nothing_is_accepted_once_it_is_over() {
        let mut drill = Drill::from_analysis(&blunder()).unwrap();
        drill.offer(Square::G7, Square::G6);
        assert_eq!(drill.offer(Square::G8, Square::F6), Offer::Illegal);
    }

    #[test]
    fn the_revealed_line_does_not_repeat_the_answer() {
        let drill = Drill::from_analysis(&blunder()).unwrap();
        let line = drill.reveal_line();
        assert_eq!(line[0], "g7g6");
        assert_eq!(
            line.iter().filter(|m| m.as_str() == "g7g6").count(),
            1,
            "the engine's variation starts with its own move: {line:?}"
        );
        assert_eq!(line, vec!["g7g6", "h5f3", "g8f6"]);
    }

    #[test]
    fn a_line_with_no_continuation_still_shows_the_move() {
        let mut analysis = blunder();
        analysis.best_line.clear();
        let drill = Drill::from_analysis(&analysis).unwrap();
        assert_eq!(drill.reveal_line(), vec!["g7g6"]);
    }

    #[test]
    fn a_move_matching_what_was_played_is_not_an_exercise() {
        let mut analysis = blunder();
        analysis.best = analysis.played.clone();
        assert!(Drill::from_analysis(&analysis).is_none());
    }

    #[test]
    fn an_unreconstructable_position_yields_no_exercise() {
        let mut analysis = blunder();
        analysis.setup_fen = "not a fen".into();
        assert!(Drill::from_analysis(&analysis).is_none());

        let mut analysis = blunder();
        analysis.setup_move = "z9z9".into();
        assert!(Drill::from_analysis(&analysis).is_none());
    }

    #[test]
    fn an_answer_that_is_not_legal_here_yields_no_exercise() {
        let mut analysis = blunder();
        analysis.best = "a1a8".into();
        assert!(Drill::from_analysis(&analysis).is_none());
    }
}
