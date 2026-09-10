//! Committing a line before the board moves.
//!
//! The puzzle trainer answers one move at a time, immediately. That is
//! guess-and-check: a puzzle can be finished without ever having been
//! calculated, the solve time falls, the measured numbers improve, and nothing
//! has touched the skill of holding a position in your head. It is the habit
//! this application exists to break and it was training it.
//!
//! Here the board is frozen, the whole line is written out — both sides, since
//! predicting the reply is most of calculating — and nothing moves until it is
//! committed. Then it says exactly which ply broke.
//!
//! The figure it produces is one the application did not have: how many plies
//! deep the line is right before the first mistake. That is the thing being
//! trained, stated as a number.

use shakmaty::san::San;
use shakmaty::{Chess, Position};

/// Where a declared line stopped agreeing with the solution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Break {
    /// Ply within the declaration, counting from one.
    pub ply: usize,
    pub wrote: String,
    pub expected: String,
}

/// What a declaration was worth.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Verdict {
    /// Plies correct from the start. The number being trained.
    pub depth: usize,
    pub declared: usize,
    pub total: usize,
    /// Whether the line was seen through to the end.
    pub complete: bool,
    pub broke: Option<Break>,
}

impl Verdict {
    /// One line, for the panel.
    pub fn describe(&self) -> String {
        let plies = |n: usize| if n == 1 { "ply" } else { "plies" };
        match &self.broke {
            None if self.complete => {
                format!("The whole line — {} {}.", self.depth, plies(self.depth))
            }
            None => format!("Right as far as it goes — {} of {}.", self.depth, self.total),
            Some(broke) => format!(
                "{} {}. Ply {}: you wrote {}, it is {}.",
                self.depth,
                plies(self.depth),
                broke.ply,
                broke.wrote,
                broke.expected
            ),
        }
    }
}

/// Read a written line against the solution.
///
/// `solution` is the puzzle's line from the position on the board, in UCI, both
/// sides alternating. `written` is free text: move numbers, dots and a result
/// are skipped, exactly as the opening book already does when reading its own
/// lines.
pub fn check(initial: &Chess, solution: &[String], written: &str) -> Verdict {
    let mut verdict = Verdict {
        total: solution.len(),
        ..Verdict::default()
    };
    let mut position = initial.clone();

    for raw in written.split_whitespace() {
        if skip(raw) {
            continue;
        }
        let token = strip_number(raw);
        if token.is_empty() {
            continue;
        }
        verdict.declared += 1;
        let at = verdict.depth;

        let Some(want) = solution.get(at) else {
            // Written past the end of the line. Not wrong so much as extra,
            // but it is not depth either.
            verdict.broke = Some(Break {
                ply: verdict.declared,
                wrote: token.to_owned(),
                expected: "the line ends".to_owned(),
            });
            return verdict;
        };

        let played = token
            .parse::<San>()
            .ok()
            .and_then(|san| san.to_move(&position).ok());
        let Some(played) = played else {
            verdict.broke = Some(Break {
                ply: verdict.declared,
                wrote: token.to_owned(),
                expected: san_of(&position, want),
            });
            return verdict;
        };

        let matches = shakmaty::uci::UciMove::from_standard(played).to_string() == *want;
        // A mate is a mate. The recorded line is one way to finish; anything
        // that ends the game on the spot has finished it, which is the rule
        // the move-by-move trainer already applies.
        let mut after = position.clone();
        after.play_unchecked(played);
        let mated = after.is_checkmate();

        if !matches && !mated {
            verdict.broke = Some(Break {
                ply: verdict.declared,
                wrote: token.to_owned(),
                expected: san_of(&position, want),
            });
            return verdict;
        }

        verdict.depth += 1;
        position = after;
        if mated {
            verdict.complete = true;
            return verdict;
        }
    }

    verdict.complete = verdict.depth == solution.len() && verdict.depth > 0;
    verdict
}

/// A move number glued to the front of the move, which is how it is actually
/// written: "1.Rb8", "23...Qd8". Only the number comes off.
fn strip_number(token: &str) -> &str {
    let digits = token.len() - token.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return token;
    }
    let after = &token[digits..];
    let dots = after.len() - after.trim_start_matches('.').len();
    if dots == 0 {
        return token;
    }
    &after[dots..]
}

/// Move numbers, dots and results are not moves.
fn skip(token: &str) -> bool {
    token.is_empty()
        || matches!(token, "1-0" | "0-1" | "1/2-1/2" | "*")
        || token
            .trim_end_matches('.')
            .chars()
            .all(|c| c.is_ascii_digit())
}

fn san_of(position: &Chess, uci: &str) -> String {
    uci.parse::<shakmaty::uci::UciMove>()
        .ok()
        .and_then(|parsed| parsed.to_move(position).ok())
        .map(|mv| shakmaty::san::SanPlus::from_move(position.clone(), mv).to_string())
        .unwrap_or_else(|| uci.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use shakmaty::fen::Fen;
    use shakmaty::CastlingMode;

    fn position(fen: &str) -> Chess {
        fen.parse::<Fen>()
            .expect("a legal fen")
            .into_position(CastlingMode::Standard)
            .expect("a legal position")
    }

    /// Back-rank mate in one: Rb8 finishes it.
    fn mate_in_one() -> (Chess, Vec<String>) {
        (
            position("6k1/5ppp/8/8/8/8/5PPP/1R4K1 w - - 0 1"),
            vec!["b1b8".to_owned()],
        )
    }

    #[test]
    fn a_whole_line_written_correctly_is_complete() {
        let (start, line) = mate_in_one();
        let verdict = check(&start, &line, "Rb8#");
        assert_eq!(verdict.depth, 1);
        assert!(verdict.complete, "{}", verdict.describe());
        assert_eq!(verdict.broke, None);
    }

    /// Move numbers and dots are notation, not moves, and a player writing a
    /// line writes them without thinking.
    #[test]
    fn move_numbers_and_dots_are_not_counted() {
        let (start, line) = mate_in_one();
        for written in ["1. Rb8#", "1.Rb8#", "23... Rb8#", "1. Rb8# 1-0"] {
            let verdict = check(&start, &line, written);
            assert_eq!(verdict.declared, 1, "{written:?} declared {} moves", verdict.declared);
            assert!(verdict.complete, "{written:?}: {}", verdict.describe());
        }
    }

    /// The number being trained is how far the line was right, and the report
    /// has to name the ply that broke rather than only failing.
    #[test]
    fn depth_is_counted_up_to_the_first_mistake() {
        let start = position("r5k1/5ppp/8/8/8/8/5PPP/1R4K1 w - - 0 1");
        // Rb8+ is met by Rxb8; the recorded line continues there.
        let line = vec!["b1b8".to_owned(), "a8b8".to_owned()];

        let verdict = check(&start, &line, "Rb8+ Kh8");
        assert_eq!(verdict.depth, 1, "the first ply was right");
        let broke = verdict.broke.as_ref().expect("a break");
        assert_eq!(broke.ply, 2);
        assert_eq!(broke.wrote, "Kh8");
        // Rxb8 is not check: the white king is on g1, not the eighth rank.
        assert_eq!(broke.expected, "Rxb8");
        assert!(!verdict.complete);
        assert!(verdict.describe().contains("Rxb8"), "{}", verdict.describe());
    }

    #[test]
    fn wrong_at_the_very_first_move_is_no_depth_at_all() {
        let (start, line) = mate_in_one();
        let verdict = check(&start, &line, "Kf1");
        assert_eq!(verdict.depth, 0);
        assert_eq!(verdict.broke.expect("a break").ply, 1);
    }

    /// Notation that will not parse, and notation that parses but is not legal
    /// here, both have to be reported rather than silently skipped.
    #[test]
    fn unreadable_and_unplayable_moves_are_both_refused() {
        let (start, line) = mate_in_one();
        for written in ["Qxz9", "Nf3"] {
            let verdict = check(&start, &line, written);
            assert_eq!(verdict.depth, 0, "{written:?}");
            assert_eq!(
                verdict.broke.as_ref().expect("a break").wrote,
                written,
                "the report must quote what was written"
            );
        }
    }

    /// A mate the recorded line did not choose still ends the game.
    #[test]
    fn an_alternative_mate_finishes_the_line() {
        // Two mates available: Rb8 and Rd8. The line records only one.
        let start = position("6k1/5ppp/8/8/8/8/5PPP/1R1R2K1 w - - 0 1");
        let line = vec!["b1b8".to_owned()];
        let verdict = check(&start, &line, "Rd8#");
        assert!(
            verdict.complete,
            "an alternative mate was marked wrong: {}",
            verdict.describe()
        );
        assert_eq!(verdict.depth, 1);
    }

    /// Writing past the end is not extra credit, and must not be counted as
    /// depth the solver did not have.
    #[test]
    fn writing_past_the_end_of_the_line_stops_there() {
        let (start, line) = mate_in_one();
        let verdict = check(&start, &line, "Rb8# Kf1 Kf2");
        assert_eq!(verdict.depth, 1);
        assert!(verdict.complete);
    }

    /// Nothing written is nothing seen — never a pass.
    #[test]
    fn an_empty_declaration_is_no_depth() {
        let (start, line) = mate_in_one();
        let verdict = check(&start, &line, "   ");
        assert_eq!(verdict.depth, 0);
        assert_eq!(verdict.declared, 0);
        assert!(!verdict.complete, "an empty line must not read as complete");
    }
}
