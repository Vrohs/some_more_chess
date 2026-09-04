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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    /// The move the engine preferred. The opponent's answer has been played
    /// for you, and `finished` says whether the line is now complete.
    Correct {
        reply: Option<String>,
        finished: bool,
    },
    /// A legal move, but not the one being looked for. When `revealed` is set
    /// the answer was played anyway, so the exercise can carry on.
    Wrong {
        revealed: Option<String>,
        reply: Option<String>,
        finished: bool,
    },
    /// Not a legal move at all; nothing happened.
    Illegal,
}

pub struct Drill {
    /// The position in front of the solver right now, which advances as the
    /// line is worked through.
    position: Chess,
    /// The whole line: the move that should have been played, the engine's
    /// answer, the next move, and so on.
    line: Vec<String>,
    /// Index of the move now expected.
    index: usize,
    misses: u32,
    finished: bool,
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
        // The engine reports its variation starting with its own choice, so the
        // answer is not repeated when the two are stitched together.
        let mut line = vec![analysis.best.clone()];
        line.extend(
            analysis
                .best_line
                .iter()
                .skip_while(|m| m.as_str() == analysis.best)
                .cloned(),
        );

        // Every move in the line must be playable, or the walkthrough would
        // stop halfway with no explanation.
        let mut probe = position.clone();
        let mut playable = 0;
        for uci in &line {
            let Ok(parsed) = uci.parse::<UciMove>() else {
                break;
            };
            let Ok(mv) = parsed.to_move(&probe) else {
                break;
            };
            probe.play_unchecked(mv);
            playable += 1;
        }
        if playable == 0 {
            return None;
        }
        line.truncate(playable);

        Some(Self {
            position,
            line,
            index: 0,
            misses: 0,
            finished: false,
        })
    }

    pub fn position(&self) -> &Chess {
        &self.position
    }

    /// The move being looked for now.
    pub fn expected(&self) -> Option<&str> {
        self.line.get(self.index).map(String::as_str)
    }

    pub fn misses(&self) -> u32 {
        self.misses
    }

    pub fn is_solved(&self) -> bool {
        self.finished
    }

    /// How far through the line the solver is, as (done, total player moves).
    pub fn progress(&self) -> (usize, usize) {
        let total = self.line.len().div_ceil(2);
        (self.index.div_ceil(2), total)
    }

    /// Offer a move by the squares it was dragged between.
    pub fn offer(&mut self, from: Square, to: Square) -> Offer {
        if self.finished {
            return Offer::Illegal;
        }
        let Some(expected) = self.expected().map(str::to_owned) else {
            self.finished = true;
            return Offer::Illegal;
        };
        let Some(mv) = find_move(&self.position, from, to, Some(&expected)) else {
            return Offer::Illegal;
        };

        if UciMove::from_standard(mv).to_string() == expected {
            self.misses = 0;
            let (reply, finished) = self.advance();
            return Offer::Correct { reply, finished };
        }

        self.misses += 1;
        if self.misses < REVEAL_AFTER_MISSES {
            return Offer::Wrong {
                revealed: None,
                reply: None,
                finished: false,
            };
        }
        // Shown, then played, so the walkthrough carries on rather than ending
        // on a failure.
        self.misses = 0;
        let (reply, finished) = self.advance();
        Offer::Wrong {
            revealed: Some(expected),
            reply,
            finished,
        }
    }

    /// Play the expected move and the answer to it.
    fn advance(&mut self) -> (Option<String>, bool) {
        self.play_at_index();
        let reply = self.line.get(self.index).cloned();
        if reply.is_some() {
            self.play_at_index();
        }
        self.finished = self.index >= self.line.len();
        (reply, self.finished)
    }

    fn play_at_index(&mut self) {
        let Some(uci) = self.line.get(self.index).cloned() else {
            return;
        };
        if let Ok(mv) = uci.parse::<UciMove>().and_then(|p| {
            p.to_move(&self.position)
                .map_err(|_| shakmaty::uci::ParseUciMoveError)
        }) {
            self.position.play_unchecked(mv);
        }
        self.index += 1;
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
    /// The move that should have been played is g7g6, and the engine's line
    /// continues 4.Qf3 Nf6.
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
        assert_eq!(drill.expected(), Some("g7g6"));
        assert_eq!(
            drill.position().turn(),
            shakmaty::Color::Black,
            "the exercise must be posed to the side that erred"
        );
        assert!(!drill.is_solved());
        assert_eq!(drill.progress(), (0, 2), "two moves to find");
    }

    #[test]
    fn finding_the_move_plays_the_answer_and_asks_for_the_next() {
        let mut drill = Drill::from_analysis(&blunder()).unwrap();
        let offer = drill.offer(Square::G7, Square::G6);
        assert_eq!(
            offer,
            Offer::Correct {
                reply: Some("h5f3".into()),
                finished: false
            },
            "the opponent's answer should be played for the solver"
        );
        assert!(!drill.is_solved(), "the line is not over");
        assert_eq!(
            drill.expected(),
            Some("g8f6"),
            "now it should ask for the next move in the line"
        );
        assert_eq!(drill.progress(), (1, 2));
    }

    #[test]
    fn working_through_the_whole_line_finishes_it() {
        let mut drill = Drill::from_analysis(&blunder()).unwrap();
        drill.offer(Square::G7, Square::G6);
        let last = drill.offer(Square::G8, Square::F6);
        assert_eq!(
            last,
            Offer::Correct {
                reply: None,
                finished: true
            }
        );
        assert!(drill.is_solved());
        assert_eq!(drill.expected(), None);
    }

    #[test]
    fn one_wrong_move_does_not_give_the_answer_away() {
        let mut drill = Drill::from_analysis(&blunder()).unwrap();
        assert_eq!(
            drill.offer(Square::G8, Square::F6),
            Offer::Wrong {
                revealed: None,
                reply: None,
                finished: false
            }
        );
        assert_eq!(drill.expected(), Some("g7g6"), "still asking for the same move");
    }

    #[test]
    fn the_second_wrong_move_shows_it_and_carries_on() {
        let mut drill = Drill::from_analysis(&blunder()).unwrap();
        drill.offer(Square::G8, Square::F6);
        let offer = drill.offer(Square::D7, Square::D6);
        assert_eq!(
            offer,
            Offer::Wrong {
                revealed: Some("g7g6".into()),
                reply: Some("h5f3".into()),
                finished: false
            },
            "the answer is shown, played, and the walkthrough continues"
        );
        assert_eq!(drill.expected(), Some("g8f6"));
        assert_eq!(drill.misses(), 0, "the count resets for the next move");
    }

    #[test]
    fn an_illegal_drag_is_not_counted_as_a_wrong_answer() {
        let mut drill = Drill::from_analysis(&blunder()).unwrap();
        assert_eq!(drill.offer(Square::A1, Square::H8), Offer::Illegal);
        assert_eq!(drill.misses(), 0, "a misdrag must not use up an attempt");
    }

    #[test]
    fn nothing_is_accepted_once_the_line_is_done() {
        let mut drill = Drill::from_analysis(&blunder()).unwrap();
        drill.offer(Square::G7, Square::G6);
        drill.offer(Square::G8, Square::F6);
        assert_eq!(drill.offer(Square::D7, Square::D6), Offer::Illegal);
    }

    #[test]
    fn a_line_with_no_continuation_is_a_single_move_exercise() {
        let mut analysis = blunder();
        analysis.best_line.clear();
        let mut drill = Drill::from_analysis(&analysis).unwrap();
        assert_eq!(drill.progress(), (0, 1));
        assert_eq!(
            drill.offer(Square::G7, Square::G6),
            Offer::Correct {
                reply: None,
                finished: true
            }
        );
    }

    #[test]
    fn a_line_that_cannot_be_played_out_is_truncated_not_broken() {
        let mut analysis = blunder();
        // A legal answer followed by nonsense.
        analysis.best_line = vec!["g7g6".into(), "a1a8".into()];
        let mut drill = Drill::from_analysis(&analysis).unwrap();
        assert_eq!(
            drill.offer(Square::G7, Square::G6),
            Offer::Correct {
                reply: None,
                finished: true
            },
            "the unplayable tail is dropped rather than stalling the exercise"
        );
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
        analysis.best_line = vec!["a1a8".into()];
        assert!(Drill::from_analysis(&analysis).is_none());
    }
}
