//! Puzzle representation and the solve state machine.
//!
//! Lichess puzzles are stored as a position *before* the losing move: the FEN
//! is the position to which `moves[0]` is applied by the opponent, after which
//! the solver plays `moves[1]`, the opponent replies with `moves[2]`, and so on.

use anyhow::{anyhow, bail, Context, Result};
use shakmaty::fen::Fen;
use shakmaty::uci::UciMove;
use shakmaty::{CastlingMode, Chess, Move, Position};

/// A single puzzle as published in the Lichess open database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Puzzle {
    pub id: String,
    pub fen: String,
    /// UCI moves, opponent first, then alternating.
    pub moves: Vec<String>,
    pub rating: u32,
    pub rating_deviation: u32,
    pub popularity: i32,
    pub nb_plays: u32,
    pub themes: Vec<String>,
    pub game_url: String,
    pub opening_tags: Vec<String>,
}

/// The result of offering a move to an in-progress attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveOutcome {
    /// Accepted; the opponent answered with this move.
    Continued(Move),
    /// Accepted, and the puzzle is complete.
    Solved,
    /// Not the solution. The attempt is left untouched.
    Wrong,
}

/// An in-progress solve.
#[derive(Debug, Clone)]
pub struct Attempt {
    position: Chess,
    line: Vec<String>,
    ply: usize,
}

impl Attempt {
    /// Set up an attempt, applying the opponent's opening move so the position
    /// is the one the solver is actually shown.
    pub fn new(puzzle: &Puzzle) -> Result<Self> {
        let fen: Fen = puzzle
            .fen
            .parse()
            .with_context(|| format!("puzzle {}: invalid FEN", puzzle.id))?;
        let mut position: Chess = fen
            .into_position(CastlingMode::Standard)
            .map_err(|e| anyhow!("puzzle {}: illegal position: {e}", puzzle.id))?;

        let opening = puzzle
            .moves
            .first()
            .ok_or_else(|| anyhow!("puzzle {}: no moves", puzzle.id))?;
        let mv = parse_uci(&position, opening)
            .with_context(|| format!("puzzle {}: illegal opening move {opening}", puzzle.id))?;
        position.play_unchecked(mv);

        Ok(Self {
            position,
            line: puzzle.moves.clone(),
            ply: 1,
        })
    }

    /// The position the solver is looking at.
    pub fn position(&self) -> &Chess {
        &self.position
    }

    /// The move the solution expects next, if the attempt is unfinished.
    pub fn expected(&self) -> Option<&str> {
        self.line.get(self.ply).map(String::as_str)
    }

    /// True once every solver move has been played.
    pub fn is_complete(&self) -> bool {
        self.ply >= self.line.len()
    }

    /// Interpret a UCI string in the current position.
    pub fn parse_move(&self, uci: &str) -> Result<Move> {
        parse_uci(&self.position, uci)
    }

    /// Offer a move. A wrong move leaves the attempt unchanged so the caller
    /// may record the failure and still show the position.
    pub fn play(&mut self, mv: &Move) -> Result<MoveOutcome> {
        let expected = match self.expected() {
            Some(uci) => uci.to_owned(),
            None => bail!("attempt is already complete"),
        };

        if !self.accepts(mv, &expected)? {
            return Ok(MoveOutcome::Wrong);
        }

        self.position.play_unchecked(*mv);
        self.ply += 1;

        // A solver move that ends the game finishes the puzzle even when the
        // recorded line is longer, which happens when an alternative mate was
        // accepted above.
        if self.is_complete() || self.position.is_game_over() {
            self.ply = self.line.len();
            return Ok(MoveOutcome::Solved);
        }

        let reply_uci = self.line[self.ply].clone();
        let reply = parse_uci(&self.position, &reply_uci)
            .with_context(|| format!("illegal reply {reply_uci} in solution line"))?;
        self.position.play_unchecked(reply);
        self.ply += 1;
        Ok(MoveOutcome::Continued(reply))
    }

    /// Whether a move counts as correct. Besides the recorded solution, any
    /// move delivering immediate checkmate is accepted — Lichess does the same,
    /// and rejecting a forced mate would record a false failure and corrupt the
    /// fluency measurement.
    fn accepts(&self, mv: &Move, expected: &str) -> Result<bool> {
        if UciMove::from_standard(*mv).to_string() == expected {
            return Ok(true);
        }
        let mut probe = self.position.clone();
        if !probe.is_legal(*mv) {
            return Ok(false);
        }
        probe.play_unchecked(*mv);
        Ok(probe.is_checkmate())
    }
}

fn parse_uci(position: &Chess, uci: &str) -> Result<Move> {
    let parsed: UciMove = uci.parse().with_context(|| format!("bad UCI move {uci}"))?;
    parsed
        .to_move(position)
        .map_err(|e| anyhow!("move {uci} is not legal here: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Black king steps to h8; white mates with Rb8. A second rook on e1 gives
    /// an equally valid mate on e8, which the solver is allowed to play.
    fn two_mates() -> Puzzle {
        Puzzle {
            id: "test0001".into(),
            fen: "6k1/5ppp/8/8/8/8/5PPP/1R2R1K1 b - - 0 1".into(),
            moves: vec!["g8h8".into(), "b1b8".into()],
            rating: 1200,
            rating_deviation: 75,
            popularity: 90,
            nb_plays: 1000,
            themes: vec!["mateIn1".into(), "backRankMate".into()],
            game_url: "https://lichess.org/test".into(),
            opening_tags: vec![],
        }
    }

    #[test]
    fn setup_applies_the_opponents_move() {
        let attempt = Attempt::new(&two_mates()).unwrap();
        // After 1... Kh8 it is White to move.
        assert_eq!(attempt.position().turn(), shakmaty::Color::White);
        assert_eq!(attempt.expected(), Some("b1b8"));
        assert!(!attempt.is_complete());
    }

    #[test]
    fn the_recorded_solution_solves_the_puzzle() {
        let puzzle = two_mates();
        let mut attempt = Attempt::new(&puzzle).unwrap();
        let mv = attempt.parse_move("b1b8").unwrap();
        assert_eq!(attempt.play(&mv).unwrap(), MoveOutcome::Solved);
        assert!(attempt.is_complete());
    }

    #[test]
    fn an_alternative_mate_is_accepted() {
        let puzzle = two_mates();
        let mut attempt = Attempt::new(&puzzle).unwrap();
        let mv = attempt.parse_move("e1e8").unwrap();
        assert_eq!(attempt.play(&mv).unwrap(), MoveOutcome::Solved);
    }

    #[test]
    fn a_wrong_move_is_rejected_and_changes_nothing() {
        let puzzle = two_mates();
        let mut attempt = Attempt::new(&puzzle).unwrap();
        let before = attempt.position().clone();
        let mv = attempt.parse_move("b1b7").unwrap();
        assert_eq!(attempt.play(&mv).unwrap(), MoveOutcome::Wrong);
        assert_eq!(attempt.position(), &before);
        assert_eq!(attempt.expected(), Some("b1b8"));
    }

    #[test]
    fn a_multi_move_line_plays_the_opponents_reply() {
        // 1... Kh8 2. Rb7 (threat) h6 3. Rb8#
        let puzzle = Puzzle {
            moves: vec!["g8h8".into(), "b1b7".into(), "h7h6".into(), "b7b8".into()],
            ..two_mates()
        };
        let mut attempt = Attempt::new(&puzzle).unwrap();
        let mv = attempt.parse_move("b1b7").unwrap();
        match attempt.play(&mv).unwrap() {
            MoveOutcome::Continued(reply) => {
                assert_eq!(UciMove::from_standard(reply).to_string(), "h7h6");
            }
            other => panic!("expected the opponent to reply, got {other:?}"),
        }
        assert_eq!(attempt.expected(), Some("b7b8"));
        let mate = attempt.parse_move("b7b8").unwrap();
        assert_eq!(attempt.play(&mate).unwrap(), MoveOutcome::Solved);
    }

    #[test]
    fn a_malformed_fen_is_reported_with_the_puzzle_id() {
        let puzzle = Puzzle {
            fen: "not a fen".into(),
            ..two_mates()
        };
        let err = Attempt::new(&puzzle).unwrap_err().to_string();
        assert!(err.contains("test0001"), "unhelpful error: {err}");
    }
}
