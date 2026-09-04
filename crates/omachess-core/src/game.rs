//! A game in progress against an engine.

use anyhow::{anyhow, bail, Context, Result};
use shakmaty::fen::Fen;
use shakmaty::san::San;
use shakmaty::uci::UciMove;
use shakmaty::{Chess, Color, EnPassantMode, KnownOutcome, Move, Position, Role, Square};

/// The standard starting position.
pub const START_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

/// How far above the player the engine is set. Enough to be a stretch, not
/// enough to be a wall.
pub const OPPONENT_MARGIN: f64 = 100.0;

/// Update playing strength after a game against an engine of known strength.
///
/// A plain Elo update, so beating a stronger opponent moves it more than
/// beating a weaker one, and the engine's level follows actual results rather
/// than how well puzzles are going.
pub fn next_play_rating(current: f64, opponent: f64, result: &str) -> f64 {
    let score = match result {
        "won" => 1.0,
        "drawn" => 0.5,
        _ => 0.0,
    };
    let expected = 1.0 / (1.0 + 10f64.powf((opponent - current) / 400.0));
    let updated = current + 24.0 * (score - expected);
    updated.max(f64::from(crate::engine::MIN_LIMITED_ELO))
}

/// Standard piece values, for the material readout.
///
/// Material is shown during a game where an evaluation is not, because it is
/// already visible on the board — counting it is a convenience, not a hint.
pub fn material_balance(position: &Chess, player: Color) -> i32 {
    let board = position.board();
    let mut balance = 0;
    for index in 0..64u32 {
        let square = Square::new(index);
        let Some(piece) = board.piece_at(square) else {
            continue;
        };
        let value = match piece.role {
            Role::Pawn => 1,
            Role::Knight | Role::Bishop => 3,
            Role::Rook => 5,
            Role::Queen => 9,
            Role::King => 0,
        };
        balance += if piece.color == player { value } else { -value };
    }
    balance
}

/// Whose move it is, or that the game is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    Player,
    Engine,
    Finished,
}

pub fn whose_turn(game: &Game) -> Turn {
    if game.outcome().is_some() {
        Turn::Finished
    } else if game.position().turn() == game.player() {
        Turn::Player
    } else {
        Turn::Engine
    }
}

/// Find the legal move a player means by dragging from one square to another.
///
/// Two things make this less obvious than it looks. Shakmaty represents castling
/// as king-takes-rook, so `Move::to()` is the *rook's* square — matching on that
/// alone makes castling unreachable, because nobody drags their king onto their
/// own rook. And a pawn reaching the last rank has four legal moves sharing the
/// same pair of squares.
///
/// `prefer` is a UCI string to disambiguate promotions when one is known,
/// such as a puzzle's expected move; otherwise a queen is assumed.
pub fn find_move(position: &Chess, from: Square, to: Square, prefer: Option<&str>) -> Option<Move> {
    let mover = position.turn();
    let mut candidates: Vec<Move> = position
        .legal_moves()
        .into_iter()
        .filter(|mv| {
            if mv.from() != Some(from) {
                return false;
            }
            if mv.to() == to {
                return true;
            }
            // Accept the square the king actually lands on, which is how
            // castling is played everywhere outside Chess960.
            mv.castling_side()
                .is_some_and(|side| side.king_to(mover) == to)
        })
        .collect();

    if candidates.len() <= 1 {
        return candidates.pop();
    }
    if let Some(prefer) = prefer {
        if let Some(mv) = candidates
            .iter()
            .find(|mv| UciMove::from_standard(**mv).to_string() == prefer)
        {
            return Some(*mv);
        }
    }
    candidates
        .into_iter()
        .find(|mv| mv.promotion() == Some(Role::Queen))
}

/// Why a game stopped, which is what the player actually wants told to them —
/// "you lost" is far less useful than "checkmated".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Checkmate,
    Stalemate,
    InsufficientMaterial,
    Resigned,
}

impl EndReason {
    pub fn label(self) -> &'static str {
        match self {
            EndReason::Checkmate => "Checkmate",
            EndReason::Stalemate => "Stalemate",
            EndReason::InsufficientMaterial => "Insufficient material",
            EndReason::Resigned => "Resigned",
        }
    }
}

/// How a finished game ended, phrased for the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Won,
    Lost,
    Drawn,
}

pub struct Game {
    initial_fen: String,
    position: Chess,
    /// Moves so far in UCI notation, which is what engines speak.
    moves: Vec<String>,
    /// Algebraic notation of the same moves, which is what people read.
    san: Vec<String>,
    player: Color,
    /// Set when the player gives up, which ends the game without a mate.
    resigned: bool,
}

impl Game {
    /// A new game from the starting position, with the player on `player`.
    pub fn new(player: Color) -> Self {
        Self {
            initial_fen: START_FEN.to_owned(),
            position: Chess::default(),
            moves: Vec::new(),
            san: Vec::new(),
            player,
            resigned: false,
        }
    }

    /// Start from a given position rather than the initial array, which is
    /// what a studied endgame needs.
    pub fn from_fen(player: Color, fen: &str) -> Option<Self> {
        use shakmaty::fen::Fen;
        use shakmaty::CastlingMode;
        let position: Chess = fen
            .parse::<Fen>()
            .ok()?
            .into_position(CastlingMode::Standard)
            .ok()?;
        Some(Self {
            initial_fen: fen.to_owned(),
            position,
            moves: Vec::new(),
            san: Vec::new(),
            player,
            resigned: false,
        })
    }

    pub fn position(&self) -> &Chess {
        &self.position
    }

    pub fn player(&self) -> Color {
        self.player
    }

    pub fn initial_fen(&self) -> &str {
        &self.initial_fen
    }

    pub fn moves(&self) -> &[String] {
        &self.moves
    }

    pub fn san(&self) -> &[String] {
        &self.san
    }

    pub fn fen(&self) -> String {
        Fen::from_position(&self.position, EnPassantMode::Legal).to_string()
    }

    /// True when it is the player's move and the game is unfinished.
    pub fn is_player_turn(&self) -> bool {
        self.outcome().is_none() && self.position.turn() == self.player
    }

    /// The result, or `None` while the game is still in progress.
    pub fn outcome(&self) -> Option<KnownOutcome> {
        if self.resigned {
            return Some(KnownOutcome::Decisive {
                winner: !self.player,
            });
        }
        self.position.outcome().known()
    }

    /// Why the game ended, if it has.
    pub fn end_reason(&self) -> Option<EndReason> {
        if self.resigned {
            return Some(EndReason::Resigned);
        }
        if self.position.is_checkmate() {
            return Some(EndReason::Checkmate);
        }
        if self.position.is_stalemate() {
            return Some(EndReason::Stalemate);
        }
        if self.position.is_insufficient_material() {
            return Some(EndReason::InsufficientMaterial);
        }
        None
    }

    /// The king that has been mated, for marking on the board.
    pub fn mated_king(&self) -> Option<Square> {
        self.position
            .is_checkmate()
            .then(|| self.position.board().king_of(self.position.turn()))
            .flatten()
    }

    /// Give up. A resigned game is still worth reviewing — usually more so.
    pub fn resign(&mut self) {
        self.resigned = true;
    }

    pub fn is_resigned(&self) -> bool {
        self.resigned
    }

    /// How the game ended, from the player's point of view.
    pub fn verdict(&self) -> Option<Verdict> {
        match self.outcome()? {
            KnownOutcome::Draw => Some(Verdict::Drawn),
            KnownOutcome::Decisive { winner } if winner == self.player => Some(Verdict::Won),
            KnownOutcome::Decisive { .. } => Some(Verdict::Lost),
        }
    }

    /// Interpret a UCI string in the current position.
    pub fn parse_move(&self, uci: &str) -> Result<Move> {
        let parsed: UciMove = uci.parse().with_context(|| format!("bad UCI move {uci}"))?;
        parsed
            .to_move(&self.position)
            .map_err(|e| anyhow!("move {uci} is not legal here: {e}"))
    }

    /// Play a move, recording both notations.
    pub fn play(&mut self, mv: &Move) -> Result<()> {
        if !self.position.is_legal(*mv) {
            bail!("illegal move");
        }
        self.san
            .push(San::from_move(&self.position, *mv).to_string());
        self.moves.push(UciMove::from_standard(*mv).to_string());
        self.position.play_unchecked(*mv);
        Ok(())
    }

    /// Move pairs as they would be written on a scoresheet.
    pub fn move_pairs(&self) -> Vec<(usize, String, Option<String>)> {
        self.san
            .chunks(2)
            .enumerate()
            .map(|(index, pair)| (index + 1, pair[0].clone(), pair.get(1).cloned()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(game: &mut Game, uci: &str) {
        let mv = game.parse_move(uci).unwrap();
        game.play(&mv).unwrap();
    }

    #[test]
    fn white_moves_first_and_turns_alternate() {
        let mut game = Game::new(Color::White);
        assert!(game.is_player_turn());
        play(&mut game, "e2e4");
        assert!(!game.is_player_turn());
        play(&mut game, "e7e5");
        assert!(game.is_player_turn());
    }

    #[test]
    fn playing_black_means_waiting_for_the_first_move() {
        let game = Game::new(Color::Black);
        assert!(!game.is_player_turn());
    }

    #[test]
    fn both_notations_are_recorded() {
        let mut game = Game::new(Color::White);
        play(&mut game, "e2e4");
        play(&mut game, "e7e5");
        play(&mut game, "g1f3");
        assert_eq!(game.moves(), ["e2e4", "e7e5", "g1f3"]);
        assert_eq!(game.san(), ["e4", "e5", "Nf3"]);
    }

    #[test]
    fn an_illegal_move_is_refused_and_changes_nothing() {
        let mut game = Game::new(Color::White);
        let mv = game.parse_move("e2e4").unwrap();
        game.play(&mv).unwrap();
        // The same move is no longer legal; the pawn has gone.
        assert!(game.parse_move("e2e4").is_err());
        assert_eq!(game.moves().len(), 1);
    }

    #[test]
    fn the_scholars_mate_is_a_win_for_the_player_who_gave_it() {
        let mut game = Game::new(Color::White);
        for uci in ["e2e4", "e7e5", "f1c4", "b8c6", "d1h5", "g8f6", "h5f7"] {
            play(&mut game, uci);
        }
        assert_eq!(game.verdict(), Some(Verdict::Won));
        assert!(!game.is_player_turn(), "a finished game has no turn");
    }

    #[test]
    fn the_same_mate_is_a_loss_when_you_are_on_the_other_side() {
        let mut game = Game::new(Color::Black);
        for uci in ["e2e4", "e7e5", "f1c4", "b8c6", "d1h5", "g8f6", "h5f7"] {
            play(&mut game, uci);
        }
        assert_eq!(game.verdict(), Some(Verdict::Lost));
    }

    #[test]
    fn resigning_ends_the_game_as_a_loss() {
        let mut game = Game::new(Color::White);
        play(&mut game, "e2e4");
        assert!(game.outcome().is_none());

        game.resign();
        assert_eq!(game.verdict(), Some(Verdict::Lost));
        assert!(!game.is_player_turn(), "a resigned game has no turn");
        assert!(game.is_resigned());
    }

    #[test]
    fn resigning_as_black_also_loses() {
        let mut game = Game::new(Color::Black);
        play(&mut game, "e2e4");
        game.resign();
        assert_eq!(game.verdict(), Some(Verdict::Lost));
    }

    #[test]
    fn a_resigned_game_keeps_its_moves_for_review() {
        let mut game = Game::new(Color::White);
        for uci in ["e2e4", "e7e5", "g1f3"] {
            play(&mut game, uci);
        }
        game.resign();
        assert_eq!(game.moves().len(), 3, "moves must survive for the review");
    }

    #[test]
    fn a_mate_is_reported_as_checkmate_not_merely_a_loss() {
        let mut game = Game::new(Color::White);
        for uci in ["e2e4", "e7e5", "f1c4", "b8c6", "d1h5", "g8f6", "h5f7"] {
            play(&mut game, uci);
        }
        assert_eq!(game.end_reason(), Some(EndReason::Checkmate));
        assert!(
            game.mated_king().is_some(),
            "the mated king must be locatable"
        );
    }

    #[test]
    fn resigning_is_distinguished_from_being_mated() {
        let mut game = Game::new(Color::White);
        play(&mut game, "e2e4");
        game.resign();
        assert_eq!(game.end_reason(), Some(EndReason::Resigned));
        assert_eq!(game.mated_king(), None);
    }

    #[test]
    fn an_unfinished_game_has_no_end_reason() {
        let mut game = Game::new(Color::White);
        play(&mut game, "e2e4");
        assert_eq!(game.end_reason(), None);
    }

    #[test]
    fn stalemate_is_named_as_such() {
        // Black to move, king on a8, white queen c7 and king c8-adjacent: no
        // legal move and no check.
        let fen = "k7/2Q5/1K6/8/8/8/8/8 b - - 0 1";
        let setup: shakmaty::fen::Fen = fen.parse().unwrap();
        let position: Chess = setup
            .into_position(shakmaty::CastlingMode::Standard)
            .unwrap();
        assert!(position.is_stalemate(), "test position is not stalemate");
    }

    fn position(fen: &str) -> Chess {
        fen.parse::<Fen>()
            .unwrap()
            .into_position(shakmaty::CastlingMode::Standard)
            .unwrap()
    }

    #[test]
    fn beating_the_engine_raises_playing_strength_and_losing_lowers_it() {
        let start = 1400.0;
        assert!(next_play_rating(start, 1400.0, "won") > start);
        assert!(next_play_rating(start, 1400.0, "lost") < start);
        assert_eq!(next_play_rating(start, 1400.0, "drawn"), start);
    }

    #[test]
    fn beating_a_stronger_engine_is_worth_more() {
        let strong = next_play_rating(1400.0, 1800.0, "won") - 1400.0;
        let weak = next_play_rating(1400.0, 1420.0, "won") - 1400.0;
        assert!(strong > weak * 1.5, "{strong} should far exceed {weak}");
    }

    #[test]
    fn playing_strength_never_falls_below_what_the_engine_can_do() {
        let mut rating = f64::from(crate::engine::MIN_LIMITED_ELO);
        for _ in 0..40 {
            rating = next_play_rating(rating, 2000.0, "lost");
        }
        assert_eq!(rating, f64::from(crate::engine::MIN_LIMITED_ELO));
    }

    #[test]
    fn material_is_level_at_the_start_and_signed_from_the_player() {
        let game = Game::new(Color::White);
        assert_eq!(material_balance(game.position(), Color::White), 0);

        // White a clean queen up.
        let pos = position("4k3/8/8/8/8/8/8/3QK3 w - - 0 1");
        assert_eq!(material_balance(&pos, Color::White), 9);
        assert_eq!(
            material_balance(&pos, Color::Black),
            -9,
            "the same position must read the other way for the other side"
        );
    }

    #[test]
    fn kings_are_not_counted_as_material() {
        let pos = position("4k3/8/8/8/8/8/8/4K3 w - - 0 1");
        assert_eq!(material_balance(&pos, Color::White), 0);
    }

    #[test]
    fn the_turn_moves_between_the_player_and_the_engine() {
        let mut game = Game::new(Color::White);
        assert_eq!(whose_turn(&game), Turn::Player);
        play(&mut game, "e2e4");
        assert_eq!(whose_turn(&game), Turn::Engine);
    }

    #[test]
    fn a_finished_game_belongs_to_neither_side() {
        let mut game = Game::new(Color::White);
        for uci in ["e2e4", "e7e5", "f1c4", "b8c6", "d1h5", "g8f6", "h5f7"] {
            play(&mut game, uci);
        }
        assert_eq!(whose_turn(&game), Turn::Finished);
        game = Game::new(Color::White);
        play(&mut game, "e2e4");
        game.resign();
        assert_eq!(whose_turn(&game), Turn::Finished, "resigning ends it too");
    }

    #[test]
    fn castling_is_found_by_dragging_the_king_two_squares() {
        // Shakmaty calls this king-takes-rook, so the naive match fails here.
        let pos = position("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1");
        let short = find_move(&pos, Square::E1, Square::G1, None)
            .expect("kingside castling must be reachable by dragging to g1");
        assert!(short.is_castle());
        let long = find_move(&pos, Square::E1, Square::C1, None)
            .expect("queenside castling must be reachable by dragging to c1");
        assert!(long.is_castle());
    }

    #[test]
    fn castling_is_also_found_by_dragging_king_onto_rook() {
        let pos = position("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1");
        assert!(find_move(&pos, Square::E1, Square::H1, None).is_some_and(|mv| mv.is_castle()));
    }

    #[test]
    fn black_castles_to_its_own_squares() {
        let pos = position("r3k2r/8/8/8/8/8/8/R3K2R b KQkq - 0 1");
        assert!(find_move(&pos, Square::E8, Square::G8, None).is_some_and(|mv| mv.is_castle()));
        assert!(find_move(&pos, Square::E8, Square::C8, None).is_some_and(|mv| mv.is_castle()));
    }

    #[test]
    fn en_passant_is_found_and_recognised_as_a_capture() {
        // White pawn on e5, Black has just played d7-d5.
        let pos = position("4k3/8/8/3pP3/8/8/8/4K3 w - d6 0 2");
        let mv =
            find_move(&pos, Square::E5, Square::D6, None).expect("en passant must be reachable");
        assert!(mv.is_en_passant());
        assert!(mv.is_capture(), "en passant is a capture");
    }

    #[test]
    fn promotion_defaults_to_a_queen() {
        let pos = position("4k3/P7/8/8/8/8/8/4K3 w - - 0 1");
        let mv = find_move(&pos, Square::A7, Square::A8, None).expect("a promotion");
        assert_eq!(mv.promotion(), Some(Role::Queen));
    }

    #[test]
    fn a_known_promotion_is_honoured_over_the_default() {
        let pos = position("4k3/P7/8/8/8/8/8/4K3 w - - 0 1");
        let mv = find_move(&pos, Square::A7, Square::A8, Some("a7a8n")).expect("a promotion");
        assert_eq!(mv.promotion(), Some(Role::Knight));
    }

    #[test]
    fn an_impossible_drag_finds_nothing() {
        let pos = position("4k3/8/8/8/8/8/8/4K3 w - - 0 1");
        assert!(find_move(&pos, Square::E1, Square::E8, None).is_none());
        assert!(find_move(&pos, Square::A1, Square::A2, None).is_none());
    }

    #[test]
    fn castling_survives_a_round_trip_through_uci() {
        let pos = position("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1");
        let mv = find_move(&pos, Square::E1, Square::G1, None).unwrap();
        assert_eq!(UciMove::from_standard(mv).to_string(), "e1g1");
    }

    #[test]
    fn moves_are_paired_by_move_number() {
        let mut game = Game::new(Color::White);
        for uci in ["e2e4", "e7e5", "g1f3"] {
            play(&mut game, uci);
        }
        let pairs = game.move_pairs();
        assert_eq!(pairs[0], (1, "e4".to_string(), Some("e5".to_string())));
        assert_eq!(pairs[1], (2, "Nf3".to_string(), None));
    }
}
