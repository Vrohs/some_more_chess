//! Walking through a game one move at a time.
//!
//! Analysis is not a verdict delivered at the end; it is looking at a position,
//! asking what should have been played, and then seeing what happened. That
//! needs a cursor over the game, which is what this is.

use anyhow::{anyhow, Context, Result};
use shakmaty::fen::Fen;
use shakmaty::san::San;
use shakmaty::uci::UciMove;
use shakmaty::{CastlingMode, Chess, Position, Square};

/// A game with a position for every point in it.
///
/// Positions are computed once on load rather than replayed on each step, so
/// stepping backwards costs the same as stepping forwards.
pub struct Walkthrough {
    positions: Vec<Chess>,
    moves: Vec<String>,
    san: Vec<String>,
    index: usize,
}

impl Walkthrough {
    /// Build from a starting position and a list of moves in UCI.
    pub fn new(initial_fen: &str, moves: &[String]) -> Result<Self> {
        let setup: Fen = initial_fen.parse().context("invalid starting FEN")?;
        let mut position: Chess = setup
            .into_position(CastlingMode::Standard)
            .map_err(|e| anyhow!("illegal starting position: {e}"))?;

        let mut positions = vec![position.clone()];
        let mut san = Vec::with_capacity(moves.len());
        let mut played = Vec::with_capacity(moves.len());

        for uci in moves {
            let parsed: UciMove = uci.parse().with_context(|| format!("bad move {uci}"))?;
            let Ok(mv) = parsed.to_move(&position) else {
                // A game that cannot be replayed to the end is still worth
                // studying up to the point where it stops making sense.
                break;
            };
            san.push(San::from_move(&position, mv).to_string());
            played.push(uci.clone());
            position.play_unchecked(mv);
            positions.push(position.clone());
        }

        Ok(Self {
            positions,
            moves: played,
            san,
            index: 0,
        })
    }

    /// The position now being looked at.
    pub fn position(&self) -> &Chess {
        &self.positions[self.index]
    }

    /// How many moves have been played to reach it.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Total moves in the game.
    pub fn len(&self) -> usize {
        self.moves.len()
    }

    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }

    pub fn at_start(&self) -> bool {
        self.index == 0
    }

    pub fn at_end(&self) -> bool {
        self.index >= self.moves.len()
    }

    /// Moves played so far, in algebraic notation.
    pub fn played_san(&self) -> &[String] {
        &self.san[..self.index]
    }

    /// Every move in UCI, for handing to an engine.
    pub fn moves_so_far(&self) -> &[String] {
        &self.moves[..self.index]
    }

    /// The move that produced the current position.
    pub fn last_move(&self) -> Option<(Square, Square)> {
        let uci: UciMove = self.moves.get(self.index.checked_sub(1)?)?.parse().ok()?;
        Some((uci.from()?, uci.to()?))
    }

    /// The move about to be played, in algebraic notation.
    pub fn next_san(&self) -> Option<&str> {
        self.san.get(self.index).map(String::as_str)
    }

    pub fn forward(&mut self) -> bool {
        if self.at_end() {
            return false;
        }
        self.index += 1;
        true
    }

    pub fn back(&mut self) -> bool {
        if self.at_start() {
            return false;
        }
        self.index -= 1;
        true
    }

    pub fn go_to_start(&mut self) {
        self.index = 0;
    }

    pub fn go_to_end(&mut self) {
        self.index = self.moves.len();
    }

    /// Jump to a point, clamped to the game.
    pub fn go_to(&mut self, index: usize) {
        self.index = index.min(self.moves.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::START_FEN;

    fn ruy() -> Walkthrough {
        let moves: Vec<String> = ["e2e4", "e7e5", "g1f3", "b8c6", "f1b5"]
            .iter()
            .map(|m| m.to_string())
            .collect();
        Walkthrough::new(START_FEN, &moves).unwrap()
    }

    #[test]
    fn a_new_walkthrough_starts_before_the_first_move() {
        let game = ruy();
        assert_eq!(game.index(), 0);
        assert_eq!(game.len(), 5);
        assert!(game.at_start());
        assert!(!game.at_end());
        assert_eq!(game.position(), &Chess::default());
        assert_eq!(game.next_san(), Some("e4"));
        assert_eq!(game.last_move(), None);
    }

    #[test]
    fn stepping_forward_and_back_returns_the_same_positions() {
        let mut game = ruy();
        let start = game.position().clone();
        for _ in 0..5 {
            assert!(game.forward());
        }
        assert!(game.at_end());
        let end = game.position().clone();

        for _ in 0..5 {
            assert!(game.back());
        }
        assert_eq!(game.position(), &start, "stepping back must retrace exactly");

        game.go_to_end();
        assert_eq!(game.position(), &end);
    }

    #[test]
    fn stepping_past_either_end_is_refused_rather_than_wrapping() {
        let mut game = ruy();
        assert!(!game.back(), "cannot step before the first move");
        game.go_to_end();
        assert!(!game.forward(), "cannot step past the last move");
    }

    #[test]
    fn notation_and_engine_moves_agree_on_where_we_are() {
        let mut game = ruy();
        game.go_to(3);
        assert_eq!(game.played_san(), ["e4", "e5", "Nf3"]);
        assert_eq!(game.moves_so_far(), ["e2e4", "e7e5", "g1f3"]);
        assert_eq!(game.next_san(), Some("Nc6"));
    }

    #[test]
    fn the_last_move_is_the_one_that_made_this_position() {
        let mut game = ruy();
        game.go_to(1);
        assert_eq!(
            game.last_move(),
            Some((Square::E2, Square::E4)),
            "after one move the highlight should be that move"
        );
    }

    #[test]
    fn jumping_beyond_the_game_lands_at_the_end() {
        let mut game = ruy();
        game.go_to(999);
        assert!(game.at_end());
        assert_eq!(game.index(), 5);
    }

    #[test]
    fn a_game_that_stops_making_sense_is_kept_up_to_that_point() {
        let moves: Vec<String> = ["e2e4", "e7e5", "a1a8"]
            .iter()
            .map(|m| m.to_string())
            .collect();
        let game = Walkthrough::new(START_FEN, &moves).unwrap();
        assert_eq!(game.len(), 2, "the illegal tail is dropped, the rest kept");
    }

    #[test]
    fn a_game_with_no_moves_is_still_a_position_to_look_at() {
        let game = Walkthrough::new(START_FEN, &[]).unwrap();
        assert!(game.is_empty());
        assert!(game.at_start() && game.at_end());
        assert_eq!(game.position(), &Chess::default());
    }
}
