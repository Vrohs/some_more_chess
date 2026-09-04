//! Reading games out of a PGN file.
//!
//! The play-quality measurement is only as good as the number of games behind
//! it, and a handful played inside this application is not a sample. Real
//! playing history lives in PGN exports from Lichess and Chess.com.

use anyhow::{Context, Result};
use pgn_reader::{RawTag, SanPlus, Skip, Visitor};
use shakmaty::uci::UciMove;
use shakmaty::{Chess, Color, Position};

/// One game, reduced to what the analysis needs.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedGame {
    pub white: String,
    pub black: String,
    /// The PGN result tag: "1-0", "0-1", "1/2-1/2" or "*".
    pub result: String,
    pub date: Option<String>,
    /// The game's URL, where the file gives one. The only stable identity a
    /// PGN offers, and what stops a re-import duplicating everything.
    pub site: Option<String>,
    /// Moves in UCI, which is what the engine speaks.
    pub moves: Vec<String>,
    /// Player ratings, where the file gives them. Without these the strength
    /// of the opposition is unknown and every game looks equally hard.
    pub white_elo: Option<u32>,
    pub black_elo: Option<u32>,
}

impl ImportedGame {
    /// Which side `name` played, if either. Matching is case-insensitive
    /// because sites disagree about capitalisation of the same account.
    pub fn side_of(&self, name: &str) -> Option<Color> {
        let name = name.trim().to_lowercase();
        if self.white.to_lowercase() == name {
            Some(Color::White)
        } else if self.black.to_lowercase() == name {
            Some(Color::Black)
        } else {
            None
        }
    }

    /// How strong the opposition was, from that player's point of view.
    pub fn opponent_elo(&self, side: Color) -> Option<u32> {
        match side {
            Color::White => self.black_elo,
            Color::Black => self.white_elo,
        }
    }

    /// The result from that player's point of view.
    pub fn outcome_for(&self, side: Color) -> &'static str {
        match (self.result.as_str(), side) {
            ("1-0", Color::White) | ("0-1", Color::Black) => "won",
            ("1-0", Color::Black) | ("0-1", Color::White) => "lost",
            ("1/2-1/2", _) => "drawn",
            _ => "unknown",
        }
    }
}

/// Read every game in a PGN stream.
///
/// Games whose moves cannot be replayed are skipped rather than aborting the
/// import: one malformed game in an export of thousands should not cost the
/// rest.
pub fn read_all(source: &[u8]) -> Result<Vec<ImportedGame>> {
    let mut reader = pgn_reader::Reader::new(source);
    let mut collector = Collector::default();
    let mut games = Vec::new();

    while let Some(game) = reader
        .read_game(&mut collector)
        .context("reading the PGN file")?
    {
        if let Some(game) = game {
            games.push(game);
        }
    }
    Ok(games)
}

#[derive(Default)]
struct Collector {
    white: String,
    black: String,
    result: String,
    date: Option<String>,
    site: Option<String>,
    white_elo: Option<u32>,
    black_elo: Option<u32>,
    position: Chess,
    moves: Vec<String>,
    broken: bool,
}

impl Visitor for Collector {
    type Tags = ();
    type Movetext = ();
    type Output = Option<ImportedGame>;

    fn begin_tags(&mut self) -> std::ops::ControlFlow<Self::Output, Self::Tags> {
        self.white.clear();
        self.black.clear();
        self.result.clear();
        self.date = None;
        self.site = None;
        self.position = Chess::default();
        self.moves.clear();
        self.broken = false;
        std::ops::ControlFlow::Continue(())
    }

    fn tag(
        &mut self,
        _tags: &mut Self::Tags,
        name: &[u8],
        value: RawTag<'_>,
    ) -> std::ops::ControlFlow<Self::Output> {
        let text = String::from_utf8_lossy(value.as_bytes()).into_owned();
        match name {
            b"White" => self.white = text,
            b"Black" => self.black = text,
            b"Result" => self.result = text,
            b"UTCDate" | b"Date" => self.date = Some(text),
            b"Site" | b"Link" => self.site = Some(text),
            b"WhiteElo" => self.white_elo = text.trim().parse().ok(),
            b"BlackElo" => self.black_elo = text.trim().parse().ok(),
            _ => {}
        }
        std::ops::ControlFlow::Continue(())
    }

    fn begin_movetext(
        &mut self,
        _tags: Self::Tags,
    ) -> std::ops::ControlFlow<Self::Output, Self::Movetext> {
        std::ops::ControlFlow::Continue(())
    }

    fn san(
        &mut self,
        _movetext: &mut Self::Movetext,
        san_plus: SanPlus,
    ) -> std::ops::ControlFlow<Self::Output> {
        if self.broken {
            return std::ops::ControlFlow::Continue(());
        }
        match san_plus.san.to_move(&self.position) {
            Ok(mv) => {
                self.moves.push(UciMove::from_standard(mv).to_string());
                self.position.play_unchecked(mv);
            }
            // An illegal move means the rest of this game cannot be trusted.
            Err(_) => self.broken = true,
        }
        std::ops::ControlFlow::Continue(())
    }

    /// Variations are commentary on what was not played, so they are skipped.
    fn begin_variation(
        &mut self,
        _movetext: &mut Self::Movetext,
    ) -> std::ops::ControlFlow<Self::Output, Skip> {
        std::ops::ControlFlow::Continue(Skip(true))
    }

    fn end_game(&mut self, _movetext: Self::Movetext) -> Self::Output {
        if self.broken || self.moves.is_empty() {
            return None;
        }
        Some(ImportedGame {
            white: std::mem::take(&mut self.white),
            black: std::mem::take(&mut self.black),
            result: std::mem::take(&mut self.result),
            date: self.date.take(),
            site: self.site.take(),
            moves: std::mem::take(&mut self.moves),
            white_elo: self.white_elo.take(),
            black_elo: self.black_elo.take(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[Event "Rated blitz game"]
[Site "https://lichess.org/abcd1234"]
[UTCDate "2026.08.01"]
[White "Vrohs"]
[Black "someone_else"]
[Result "0-1"]

1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 0-1

[Event "Rated blitz game"]
[Site "https://lichess.org/efgh5678"]
[UTCDate "2026.08.02"]
[White "another_player"]
[Black "Vrohs"]
[Result "0-1"]

1. d4 d5 2. c4 e6 3. Nc3 Nf6 0-1
"#;

    #[test]
    fn both_games_are_read_with_their_moves_in_engine_notation() {
        let games = read_all(SAMPLE.as_bytes()).unwrap();
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].white, "Vrohs");
        assert_eq!(
            games[0].site.as_deref(),
            Some("https://lichess.org/abcd1234")
        );
        assert_eq!(
            games[0].moves,
            ["e2e4", "e7e5", "g1f3", "b8c6", "f1b5", "a7a6", "b5a4", "g8f6"]
        );
        assert_eq!(games[1].moves.len(), 6);
    }

    #[test]
    fn the_players_side_is_found_whichever_colour_they_had() {
        let games = read_all(SAMPLE.as_bytes()).unwrap();
        assert_eq!(games[0].side_of("Vrohs"), Some(Color::White));
        assert_eq!(games[1].side_of("Vrohs"), Some(Color::Black));
        assert_eq!(games[0].side_of("nobody"), None);
    }

    #[test]
    fn name_matching_ignores_capitalisation() {
        let games = read_all(SAMPLE.as_bytes()).unwrap();
        assert_eq!(games[0].side_of("vrohs"), Some(Color::White));
        assert_eq!(games[0].side_of("  VROHS  "), Some(Color::White));
    }

    #[test]
    fn the_result_is_read_from_the_players_side() {
        let games = read_all(SAMPLE.as_bytes()).unwrap();
        assert_eq!(games[0].outcome_for(Color::White), "lost");
        assert_eq!(games[0].outcome_for(Color::Black), "won");
        assert_eq!(games[1].outcome_for(Color::Black), "won");
    }

    #[test]
    fn comments_and_variations_do_not_become_moves() {
        let annotated = r#"[White "a"]
[Black "b"]
[Result "1-0"]

1. e4 {a good move} e5 (1... c5 2. Nf3) 2. Nf3 $1 Nc6 1-0
"#;
        let games = read_all(annotated.as_bytes()).unwrap();
        assert_eq!(
            games[0].moves,
            ["e2e4", "e7e5", "g1f3", "b8c6"],
            "the sideline and comments must not be played"
        );
    }

    #[test]
    fn a_game_with_an_illegal_move_is_skipped_not_fatal() {
        // Nf6 is well-formed notation but no white knight can reach f6, so the
        // game cannot be replayed and is dropped rather than half-imported.
        let broken = r#"[White "a"]
[Black "b"]
[Result "*"]

1. e4 e5 2. Nf6 *

[White "c"]
[Black "d"]
[Result "1-0"]

1. d4 d5 1-0
"#;
        let games = read_all(broken.as_bytes()).unwrap();
        assert_eq!(games.len(), 1, "the sound game must still come through");
        assert_eq!(games[0].white, "c");
    }

    /// Castling is written O-O in notation and stored as king-takes-rook
    /// internally, so a game containing it is the one to check.
    #[test]
    fn castling_and_promotion_notation_survive_the_round_trip() {
        let game = r#"[White "Morphy"]
[Black "Duke"]
[Result "1-0"]

1. e4 e5 2. Nf3 d6 3. d4 Bg4 4. dxe5 Bxf3 5. Qxf3 dxe5 6. Bc4 Nf6 7. Qb3 Qe7
8. Nc3 c6 9. Bg5 b5 10. Nxb5 cxb5 11. Bxb5+ Nbd7 12. O-O-O Rd8 13. Rxd7 Rxd7
14. Rd1 Qe6 15. Bxd7+ Nxd7 16. Qb8+ Nxb8 17. Rd8# 1-0
"#;
        let games = read_all(game.as_bytes()).unwrap();
        assert_eq!(games.len(), 1);
        let moves = &games[0].moves;
        assert_eq!(moves.len(), 33, "the whole game must replay");
        assert!(
            moves.contains(&"e1c1".to_string()),
            "queenside castling should appear as the king's own move: {moves:?}"
        );
        assert_eq!(moves.last().map(String::as_str), Some("d1d8"), "the mate");
    }

    #[test]
    fn an_empty_file_reads_as_no_games() {
        assert!(read_all(b"").unwrap().is_empty());
    }
}
