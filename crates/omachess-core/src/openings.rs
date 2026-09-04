//! Naming the opening being played.
//!
//! Knowing that you are in the Ruy Lopez, and at which move you left the book,
//! is real knowledge rather than a hint about the position in front of you —
//! so unlike an evaluation it can be shown while the game is still running.
//!
//! The data is the Lichess opening book, which is CC0 (see
//! `assets/openings/COPYING.txt`). It stores lines in algebraic notation, so
//! each is replayed once on first use to key it by the moves an engine speaks.

use std::collections::HashMap;
use std::sync::OnceLock;

use shakmaty::san::San;
use shakmaty::uci::UciMove;
use shakmaty::{Chess, Position};

const BOOK: [&str; 5] = [
    include_str!("../../../assets/openings/a.tsv"),
    include_str!("../../../assets/openings/b.tsv"),
    include_str!("../../../assets/openings/c.tsv"),
    include_str!("../../../assets/openings/d.tsv"),
    include_str!("../../../assets/openings/e.tsv"),
];

/// A named opening line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opening {
    pub eco: String,
    pub name: String,
    /// How many plies of the game the name covers.
    pub plies: usize,
}

/// The most specific named opening matching the start of `moves`, in UCI.
///
/// Longest match wins, because the book contains both "French Defense" and its
/// many named continuations, and the specific name is the useful one.
pub fn identify(moves: &[String]) -> Option<Opening> {
    let book = book();
    let mut best: Option<Opening> = None;

    for length in 1..=moves.len().min(longest_line()) {
        let key = moves[..length].join(" ");
        if let Some((eco, name)) = book.get(&key) {
            best = Some(Opening {
                eco: eco.clone(),
                name: name.clone(),
                plies: length,
            });
        }
    }
    best
}

/// How many plies of a game are still inside the book.
pub fn book_depth(moves: &[String]) -> usize {
    identify(moves).map(|o| o.plies).unwrap_or(0)
}

fn book() -> &'static HashMap<String, (String, String)> {
    static BOOK_INDEX: OnceLock<HashMap<String, (String, String)>> = OnceLock::new();
    BOOK_INDEX.get_or_init(build_index)
}

fn longest_line() -> usize {
    static LONGEST: OnceLock<usize> = OnceLock::new();
    *LONGEST.get_or_init(|| book().keys().map(|k| k.split(' ').count()).max().unwrap_or(0))
}

fn build_index() -> HashMap<String, (String, String)> {
    let mut index = HashMap::new();
    for table in BOOK {
        for line in table.lines().skip(1) {
            let mut fields = line.split('\t');
            let (Some(eco), Some(name), Some(pgn)) = (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if let Some(key) = to_uci_key(pgn) {
                index.insert(key, (eco.to_owned(), name.to_owned()));
            }
        }
    }
    index
}

/// Replay a line in algebraic notation to get the moves an engine would speak.
fn to_uci_key(pgn: &str) -> Option<String> {
    let mut position = Chess::default();
    let mut moves = Vec::new();

    for token in pgn.split_whitespace() {
        // Skip move numbers such as "1." and "12...".
        if token.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let san: San = token.parse().ok()?;
        let mv = san.to_move(&position).ok()?;
        moves.push(UciMove::from_standard(mv).to_string());
        position.play_unchecked(mv);
    }
    (!moves.is_empty()).then(|| moves.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uci(moves: &[&str]) -> Vec<String> {
        moves.iter().map(|m| (*m).to_string()).collect()
    }

    #[test]
    fn the_book_parses_and_is_substantial() {
        assert!(
            book().len() > 3000,
            "only {} lines parsed from the book",
            book().len()
        );
    }

    #[test]
    fn a_common_opening_is_named() {
        // 1. e4 e5 2. Nf3 Nc6 3. Bb5 — the Ruy Lopez.
        let opening = identify(&uci(&["e2e4", "e7e5", "g1f3", "b8c6", "f1b5"])).expect("a name");
        assert!(
            opening.name.contains("Ruy Lopez"),
            "expected the Ruy Lopez, got {}",
            opening.name
        );
        assert_eq!(opening.eco.chars().next(), Some('C'));
    }

    #[test]
    fn the_most_specific_name_wins() {
        let short = identify(&uci(&["e2e4", "e7e6"])).expect("a name");
        let long = identify(&uci(&["e2e4", "e7e6", "d2d4", "d7d5"])).expect("a name");
        assert!(short.name.contains("French"));
        assert!(long.plies > short.plies, "a longer line must win");
    }

    #[test]
    fn leaving_the_book_stops_the_depth_growing() {
        let book_line = uci(&["e2e4", "e7e5", "g1f3", "b8c6", "f1b5"]);
        let depth = book_depth(&book_line);

        // Append something absurd; the named depth must not increase.
        let mut wandered = book_line.clone();
        wandered.extend(uci(&["a2a3", "a7a6", "h2h3"]));
        assert_eq!(book_depth(&wandered), depth);
    }

    /// The book names every legal first move — 1. a3 is Anderssen's Opening —
    /// so a game is essentially never nameless after one move.
    #[test]
    fn even_an_eccentric_first_move_is_named() {
        for (first, expected) in [
            ("a2a3", "Anderssen"),
            ("g2g4", "Grob"),
            ("b1c3", "Van Geet"),
        ] {
            let opening = identify(&uci(&[first])).unwrap_or_else(|| panic!("{first} unnamed"));
            assert!(
                opening.name.contains(expected),
                "{first} gave {}, expected {expected}",
                opening.name
            );
        }
    }

    #[test]
    fn a_line_that_leaves_the_book_keeps_the_last_name_it_earned() {
        let wandered = uci(&["e2e4", "e7e5", "g1f3", "b8c6", "f1b5", "a7a5", "h2h4"]);
        let opening = identify(&wandered).expect("the Ruy Lopez prefix is still named");
        assert!(opening.name.contains("Ruy Lopez"), "got {}", opening.name);
        assert!(
            opening.plies <= 6,
            "the name must not claim moves it does not cover, got {}",
            opening.plies
        );
    }

    #[test]
    fn an_empty_game_has_no_name() {
        assert!(identify(&[]).is_none());
        assert_eq!(book_depth(&[]), 0);
    }
}
