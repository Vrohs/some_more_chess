//! Seeing the board, before calculating on it.
//!
//! A player who cannot say whether f6 is light or dark without looking cannot
//! hold a position in their head, and a player who cannot hold a position
//! cannot calculate — they can only push a piece and see what happens. That is
//! not a gap in tactics knowledge and no number of puzzles closes it: the
//! puzzle trainer answers every move immediately, which teaches guessing and
//! checking, which is the habit in question.
//!
//! These are the rungs below calculation. Every one is answered by clicking,
//! every one is decided by geometry rather than by an engine, and every one is
//! instant — so a wrong answer is wrong for a reason the solver can see, and
//! there is nothing to fudge.
//!
//! Notation is read here long before it has to be written. The rung that asks
//! about a position after a move shows the move in notation and leaves the
//! board where it was, which is calculation in miniature: the board on screen
//! stops being the board being asked about.

use std::collections::BTreeSet;
use std::time::Duration;

use shakmaty::{attacks, Bitboard, Board, Color, Piece, Role, Square};

/// A step on the ladder. The order is the order they are climbed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rung {
    /// "f6 — light or dark?"
    SquareColour,
    /// An empty board: "click e5".
    FindSquare,
    /// A knight alone: click everywhere it can go.
    KnightReach,
    /// A bishop, rook or queen, with something in the way.
    LineReach,
    /// Two pieces: does the first attack the second?
    IsAttacked,
    /// A move given in notation, the board left where it was.
    AfterOneMove,
}

impl Rung {
    pub const LADDER: [Rung; 6] = [
        Rung::SquareColour,
        Rung::FindSquare,
        Rung::KnightReach,
        Rung::LineReach,
        Rung::IsAttacked,
        Rung::AfterOneMove,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Rung::SquareColour => "Square colour",
            Rung::FindSquare => "Find the square",
            Rung::KnightReach => "Knight reach",
            Rung::LineReach => "Line pieces",
            Rung::IsAttacked => "Is it attacked?",
            Rung::AfterOneMove => "One move ahead",
        }
    }

    /// Stored as text so the ladder can be reordered without rewriting history.
    pub fn key(self) -> &'static str {
        match self {
            Rung::SquareColour => "colour",
            Rung::FindSquare => "find",
            Rung::KnightReach => "knight",
            Rung::LineReach => "line",
            Rung::IsAttacked => "attacked",
            Rung::AfterOneMove => "ahead",
        }
    }

    pub fn from_key(key: &str) -> Option<Rung> {
        Rung::LADDER.into_iter().find(|rung| rung.key() == key)
    }

    pub fn next(self) -> Option<Rung> {
        let at = Rung::LADDER.iter().position(|r| *r == self)?;
        Rung::LADDER.get(at + 1).copied()
    }

    /// How long an answer may take and still count as seeing it.
    ///
    /// This is a speed skill. Ninety-five per cent correct at eight seconds a
    /// square is not board vision, it is arithmetic done carefully, and it will
    /// not survive being three moves deep in a variation.
    pub fn target(self) -> Duration {
        match self {
            Rung::SquareColour | Rung::FindSquare => Duration::from_secs(3),
            Rung::KnightReach | Rung::LineReach => Duration::from_secs(12),
            Rung::IsAttacked => Duration::from_secs(6),
            Rung::AfterOneMove => Duration::from_secs(20),
        }
    }
}

/// What the solver said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Light,
    Dark,
    Yes,
    No,
    Squares(BTreeSet<Square>),
}

/// One question, and the board to show while asking it.
#[derive(Debug, Clone)]
pub struct Drill {
    pub rung: Rung,
    pub prompt: String,
    /// The arrangement to display. Often not a legal chess position — a lone
    /// knight has no kings — which is why it is a `Board` and not a `Chess`.
    pub board: Board,
    /// A move written out, for the rung where the board deliberately does not
    /// match the question.
    pub shown_move: Option<String>,
    pub answer: Answer,
}

/// Whether an answer is the right one.
pub fn judge(drill: &Drill, given: &Answer) -> bool {
    drill.answer == *given
}

/// Whether this rung has been climbed.
///
/// Both halves are required. Accurate and slow is not seeing it, and fast and
/// wrong is guessing; either alone would promote someone who cannot do the
/// next rung at all.
pub const NEEDED: usize = 20;
pub const ACCURACY: f64 = 0.9;

pub fn ready_to_promote(recent: &[(bool, Duration)], rung: Rung) -> bool {
    if recent.len() < NEEDED {
        return false;
    }
    let last: &[(bool, Duration)] = &recent[recent.len() - NEEDED..];
    let right = last.iter().filter(|(correct, _)| *correct).count();
    if (right as f64) < ACCURACY * NEEDED as f64 {
        return false;
    }
    median(&last.iter().map(|(_, took)| *took).collect::<Vec<_>>()) <= rung.target()
}

fn median(times: &[Duration]) -> Duration {
    if times.is_empty() {
        return Duration::MAX;
    }
    let mut sorted = times.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() / 2]
}

/// Deal a question. Seeded, so a run can be reproduced exactly.
pub fn next(rung: Rung, seed: u64) -> Drill {
    let mut rng = Rng(seed ^ 0x9E37_79B9_7F4A_7C15);
    match rung {
        Rung::SquareColour => {
            let square = rng.square();
            Drill {
                rung,
                prompt: format!("{square} — light or dark?"),
                board: Board::empty(),
                shown_move: None,
                answer: if square.is_light() {
                    Answer::Light
                } else {
                    Answer::Dark
                },
            }
        }
        Rung::FindSquare => {
            let square = rng.square();
            Drill {
                rung,
                prompt: format!("Click {square}"),
                board: Board::empty(),
                shown_move: None,
                answer: Answer::Squares(BTreeSet::from([square])),
            }
        }
        Rung::KnightReach => {
            let from = rng.square();
            let mut board = Board::empty();
            board.set_piece_at(from, white(Role::Knight));
            Drill {
                rung,
                prompt: format!("Knight on {from}. Click everywhere it can go."),
                board,
                shown_move: None,
                answer: Answer::Squares(squares(attacks::knight_attacks(from))),
            }
        }
        Rung::LineReach => {
            let role = [Role::Bishop, Role::Rook, Role::Queen][rng.below(3) as usize];
            let from = rng.square();
            let mut board = Board::empty();
            board.set_piece_at(from, white(role));
            // Something in the way, or the rung is only a pattern to memorise.
            let mut occupied = Bitboard::from_square(from);
            let reach = attacks::attacks(from, white(role), occupied);
            let blockers: Vec<Square> = squares(reach).into_iter().collect();
            if let Some(blocker) = blockers.get(rng.below(blockers.len().max(1) as u64) as usize) {
                board.set_piece_at(*blocker, black(Role::Pawn));
                occupied |= Bitboard::from_square(*blocker);
            }
            // The blocker's own square is reachable — it can be captured.
            let answer = squares(attacks::attacks(from, white(role), occupied));
            Drill {
                rung,
                prompt: format!(
                    "{} on {from}. Click everywhere it can go, captures included.",
                    name(role)
                ),
                board,
                shown_move: None,
                answer: Answer::Squares(answer),
            }
        }
        Rung::IsAttacked => {
            let role = [Role::Bishop, Role::Rook, Role::Queen, Role::Knight]
                [rng.below(4) as usize];
            let from = rng.square();
            let occupied = Bitboard::from_square(from);
            let reach = squares(attacks::attacks(from, white(role), occupied));
            // Half the time somewhere it does reach, half the time somewhere it
            // does not, so the answer cannot be guessed from the shape.
            let target = if rng.below(2) == 0 && !reach.is_empty() {
                *reach.iter().nth(rng.below(reach.len() as u64) as usize).unwrap()
            } else {
                let mut candidate = rng.square();
                let mut guard = 0;
                while (candidate == from || reach.contains(&candidate)) && guard < 64 {
                    candidate = rng.square();
                    guard += 1;
                }
                candidate
            };
            let mut board = Board::empty();
            board.set_piece_at(from, white(role));
            if target != from {
                board.set_piece_at(target, black(Role::King));
            }
            let hits = reach.contains(&target);
            Drill {
                rung,
                prompt: format!(
                    "Does the {} on {from} attack {target}?",
                    name(role).to_lowercase()
                ),
                board,
                shown_move: None,
                answer: if hits { Answer::Yes } else { Answer::No },
            }
        }
        Rung::AfterOneMove => {
            let role = [Role::Bishop, Role::Rook, Role::Knight][rng.below(3) as usize];
            let from = rng.square();
            let occupied = Bitboard::from_square(from);
            let landings: Vec<Square> =
                squares(attacks::attacks(from, white(role), occupied)).into_iter().collect();
            let to = landings[rng.below(landings.len() as u64) as usize];

            let mut board = Board::empty();
            board.set_piece_at(from, white(role));
            // The answer is about the piece on `to`, while the board still
            // shows it on `from`. That gap is the whole exercise.
            let after = Bitboard::from_square(to);
            Drill {
                rung,
                prompt: format!(
                    "{} goes {from} to {to}. Click everywhere it attacks from there."
                ,   name(role)
                ),
                board,
                shown_move: Some(format!("{from}{to}")),
                answer: Answer::Squares(squares(attacks::attacks(to, white(role), after))),
            }
        }
    }
}

fn white(role: Role) -> Piece {
    Piece {
        color: Color::White,
        role,
    }
}

fn black(role: Role) -> Piece {
    Piece {
        color: Color::Black,
        role,
    }
}

fn name(role: Role) -> &'static str {
    match role {
        Role::Bishop => "Bishop",
        Role::Rook => "Rook",
        Role::Queen => "Queen",
        Role::Knight => "Knight",
        Role::King => "King",
        Role::Pawn => "Pawn",
    }
}

fn squares(board: Bitboard) -> BTreeSet<Square> {
    board.into_iter().collect()
}

/// A small deterministic generator. Nothing here needs to be unpredictable —
/// it needs to be reproducible, so a drill in a test is the same drill twice.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*, chosen for being four lines rather than a dependency.
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            return 0;
        }
        self.next() % bound
    }

    fn square(&mut self) -> Square {
        Square::new(self.below(64) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sq(name: &str) -> Square {
        name.parse().expect("a square")
    }

    fn answered(drill: &Drill) -> BTreeSet<Square> {
        match &drill.answer {
            Answer::Squares(set) => set.clone(),
            other => panic!("expected squares, got {other:?}"),
        }
    }

    /// Hand-checked against a real board. a1 is dark and h1 is light, which is
    /// the rule every beginner is taught and half of them still get backwards.
    #[test]
    fn square_colours_are_the_ones_on_a_real_board() {
        for (name, light) in [
            ("a1", false),
            ("h1", true),
            ("a8", true),
            ("h8", false),
            // Worked out from a1 being dark: light when file + rank is odd.
            // Four of these were written backwards the first time, by someone
            // who had just finished arguing that this rung is worth building.
            ("e4", true),
            ("d4", false),
            ("f6", false),
            ("c5", false),
        ] {
            assert_eq!(
                sq(name).is_light(),
                light,
                "{name} should be {}",
                if light { "light" } else { "dark" }
            );
        }
        // The a1-h8 diagonal is dark end to end; a8-h1 is light end to end.
        for name in ["a1", "b2", "c3", "d4", "e5", "f6", "g7", "h8"] {
            assert!(!sq(name).is_light(), "{name} is on the dark long diagonal");
        }
        for name in ["a8", "b7", "c6", "d5", "e4", "f3", "g2", "h1"] {
            assert!(sq(name).is_light(), "{name} is on the light long diagonal");
        }
    }

    /// A knight in the corner reaches two squares, on an edge four, in the
    /// middle eight. Getting this wrong would teach the wrong board.
    #[test]
    fn a_knight_reaches_what_a_knight_reaches() {
        let corner = drill_for(Rung::KnightReach, |d| d.prompt.contains("a1"));
        assert_eq!(
            answered(&corner),
            BTreeSet::from([sq("b3"), sq("c2")]),
            "a knight on a1"
        );

        let centre = drill_for(Rung::KnightReach, |d| d.prompt.contains("d4"));
        assert_eq!(answered(&centre).len(), 8, "a knight on d4");
        assert!(answered(&centre).contains(&sq("e6")));
        assert!(!answered(&centre).contains(&sq("d5")), "knights do not step");
    }

    /// A line piece stops at what is in the way — and the blocker's own square
    /// counts, because it can be taken.
    #[test]
    fn a_line_piece_stops_at_the_blocker_and_can_take_it() {
        let mut board = Board::empty();
        board.set_piece_at(sq("a1"), white(Role::Rook));
        board.set_piece_at(sq("a4"), black(Role::Pawn));
        let occupied = Bitboard::from_square(sq("a1")) | Bitboard::from_square(sq("a4"));
        let reach = squares(attacks::attacks(sq("a1"), white(Role::Rook), occupied));

        assert!(reach.contains(&sq("a3")), "up to the blocker");
        assert!(reach.contains(&sq("a4")), "the blocker itself is takeable");
        assert!(!reach.contains(&sq("a5")), "and nothing past it");
        assert!(reach.contains(&sq("h1")), "the clear rank is all reachable");
    }

    /// The rung that bridges to calculation asks about a square the piece is
    /// not standing on. If the answer described the board being shown, the
    /// exercise would be nothing at all.
    #[test]
    fn one_move_ahead_asks_about_where_the_piece_is_going() {
        for seed in 0..200u64 {
            let drill = next(Rung::AfterOneMove, seed);
            let shown = drill.shown_move.clone().expect("a move");
            let (from, to) = (sq(&shown[0..2]), sq(&shown[2..4]));
            assert_ne!(from, to);
            assert!(
                drill.board.piece_at(from).is_some(),
                "the board should still show the piece where it started"
            );
            assert!(
                drill.board.piece_at(to).is_none(),
                "the board must not have played the move"
            );

            let piece = drill.board.piece_at(from).expect("a piece");
            let after = squares(attacks::attacks(to, piece, Bitboard::from_square(to)));
            assert_eq!(
                answered(&drill),
                after,
                "seed {seed}: the answer must describe the position after the move"
            );
        }
    }

    /// Attacked and not attacked both come up, or the rung is answerable by
    /// always saying the same thing.
    #[test]
    fn the_attacked_rung_asks_both_ways() {
        let mut yes = 0;
        let mut no = 0;
        for seed in 0..200u64 {
            match next(Rung::IsAttacked, seed).answer {
                Answer::Yes => yes += 1,
                Answer::No => no += 1,
                other => panic!("expected yes or no, got {other:?}"),
            }
        }
        assert!(yes > 20 && no > 20, "lopsided: {yes} yes, {no} no");
    }

    /// Same seed, same drill. A test that cannot reproduce a question cannot
    /// investigate one that came out wrong.
    #[test]
    fn a_seed_deals_the_same_question_twice() {
        for rung in Rung::LADDER {
            let a = next(rung, 12345);
            let b = next(rung, 12345);
            assert_eq!(a.prompt, b.prompt, "{rung:?}");
            assert_eq!(a.answer, b.answer, "{rung:?}");
        }
    }

    /// Different seeds ask different things, or it is one question forever.
    #[test]
    fn the_ladder_does_not_ask_the_same_thing_every_time() {
        for rung in Rung::LADDER {
            let asked: BTreeSet<String> =
                (0..50u64).map(|seed| next(rung, seed).prompt).collect();
            assert!(asked.len() > 5, "{rung:?} only asked {} things", asked.len());
        }
    }

    /// Accurate and slow is not seeing it; fast and wrong is guessing. Either
    /// alone would promote someone who cannot do the next rung at all.
    #[test]
    fn promotion_needs_both_halves() {
        let quick = Duration::from_secs(1);
        let slow = Duration::from_secs(30);

        let all_right_and_quick: Vec<_> = (0..NEEDED).map(|_| (true, quick)).collect();
        assert!(ready_to_promote(&all_right_and_quick, Rung::SquareColour));

        let all_right_but_slow: Vec<_> = (0..NEEDED).map(|_| (true, slow)).collect();
        assert!(
            !ready_to_promote(&all_right_but_slow, Rung::SquareColour),
            "twenty right answers at half a minute each is not board vision"
        );

        let quick_but_wrong: Vec<_> = (0..NEEDED).map(|_| (false, quick)).collect();
        assert!(!ready_to_promote(&quick_but_wrong, Rung::SquareColour));

        let too_few: Vec<_> = (0..NEEDED - 1).map(|_| (true, quick)).collect();
        assert!(!ready_to_promote(&too_few, Rung::SquareColour));

        // Only the most recent answers count: twenty good ones after a bad
        // start is climbing the rung, not a lucky average.
        let mut recovered: Vec<(bool, Duration)> = (0..40).map(|_| (false, slow)).collect();
        recovered.extend((0..NEEDED).map(|_| (true, quick)));
        assert!(ready_to_promote(&recovered, Rung::SquareColour));
    }

    /// The rung keys are what gets stored, so they have to be stable and
    /// distinct, and read back as themselves.
    #[test]
    fn every_rung_stores_and_reads_back() {
        let keys: BTreeSet<&str> = Rung::LADDER.iter().map(|r| r.key()).collect();
        assert_eq!(keys.len(), Rung::LADDER.len(), "two rungs share a key");
        for rung in Rung::LADDER {
            assert_eq!(Rung::from_key(rung.key()), Some(rung));
        }
        assert_eq!(Rung::from_key("nonsense"), None);
        assert_eq!(Rung::LADDER.last().copied().unwrap().next(), None);
    }

    /// Find a dealt drill matching a condition, so a test can name the
    /// position it wants to check rather than hoping a seed produces it.
    fn drill_for(rung: Rung, want: impl Fn(&Drill) -> bool) -> Drill {
        (0..5000u64)
            .map(|seed| next(rung, seed))
            .find(|drill| want(drill))
            .expect("no seed produced the wanted drill")
    }
}
