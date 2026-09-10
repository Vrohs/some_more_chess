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

use shakmaty::san::SanPlus;
use shakmaty::{attacks, Bitboard, Board, Chess, Color, Piece, Position, Role, Square};

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
    /// A move written down: which piece does it mean?
    ReadNotation,
    /// A move played on the board: what is it called?
    WriteNotation,
    /// A real position: which of your pieces are hanging?
    Hanging,
    /// A real position, shown and then taken away.
    Recall,
}

impl Rung {
    pub const LADDER: [Rung; 10] = [
        Rung::SquareColour,
        Rung::FindSquare,
        Rung::KnightReach,
        Rung::LineReach,
        Rung::IsAttacked,
        Rung::AfterOneMove,
        Rung::ReadNotation,
        Rung::WriteNotation,
        Rung::Hanging,
        Rung::Recall,
    ];

    /// Whether this rung needs a real position rather than an arrangement made
    /// up for it. A lone knight on an empty board is a geometry exercise; the
    /// same question asked of a game you might actually be playing is chess,
    /// and the difference is most of why the low rungs do not feel like
    /// learning anything.
    pub fn wants_a_real_position(self) -> bool {
        matches!(
            self,
            Rung::Hanging | Rung::Recall | Rung::ReadNotation | Rung::WriteNotation
        )
    }

    /// Whether the answer is typed rather than clicked. Notation has to be
    /// written to be learned: reading it is recognition, and recognition is
    /// not what writing a line out of your head asks for.
    pub fn is_written(self) -> bool {
        self == Rung::WriteNotation
    }

    pub fn label(self) -> &'static str {
        match self {
            Rung::SquareColour => "Square colour",
            Rung::FindSquare => "Find the square",
            Rung::KnightReach => "Knight reach",
            Rung::LineReach => "Line pieces",
            Rung::IsAttacked => "Is it attacked?",
            Rung::AfterOneMove => "One move ahead",
            Rung::ReadNotation => "Read the notation",
            Rung::WriteNotation => "Write the notation",
            Rung::Hanging => "What is hanging",
            Rung::Recall => "From memory",
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
            Rung::ReadNotation => "read",
            Rung::WriteNotation => "write",
            Rung::Hanging => "hanging",
            Rung::Recall => "recall",
        }
    }

    pub fn from_key(key: &str) -> Option<Rung> {
        Rung::LADDER.into_iter().find(|rung| rung.key() == key)
    }

    pub fn next(self) -> Option<Rung> {
        let at = Rung::LADDER.iter().position(|r| *r == self)?;
        Rung::LADDER.get(at + 1).copied()
    }

    pub fn previous(self) -> Option<Rung> {
        let at = Rung::LADDER.iter().position(|r| *r == self)?;
        at.checked_sub(1).and_then(|below| Rung::LADDER.get(below).copied())
    }

    /// Where somebody starts who has never done this.
    ///
    /// The top, not the bottom. Asking a player who already knows the board
    /// whether f6 is light is a quiz, not learning, and twenty of them before
    /// anything interesting happens is how a ladder gets abandoned. Starting
    /// at the hardest rung and falling to where it hurts takes a handful of
    /// questions and lands on the edge of what they can actually do.
    pub fn opening_rung() -> Rung {
        *Rung::LADDER.last().expect("the ladder is not empty")
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
            Rung::ReadNotation => Duration::from_secs(10),
            Rung::WriteNotation => Duration::from_secs(15),
            Rung::Hanging => Duration::from_secs(25),
            Rung::Recall => Duration::from_secs(15),
        }
    }
}

/// Whether the rung is now too hard, and the ladder should step down.
///
/// Deliberately quicker to fall than to climb — five bad answers is enough to
/// know, twenty good ones are needed to move up. Being stuck one rung too high
/// is miserable and teaches nothing; being one too low costs a minute.
pub const FALL_AFTER: usize = 5;

pub fn should_step_down(recent: &[(bool, Duration)], rung: Rung) -> bool {
    if recent.len() < FALL_AFTER {
        return false;
    }
    let last: &[(bool, Duration)] = &recent[recent.len() - FALL_AFTER..];
    let right = last.iter().filter(|(correct, _)| *correct).count();
    if right * 2 < FALL_AFTER {
        return true;
    }
    median(&last.iter().map(|(_, took)| *took).collect::<Vec<_>>()) > rung.target() * 2
}

/// What the solver said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Light,
    Dark,
    Yes,
    No,
    Squares(BTreeSet<Square>),
    /// A move written the way a player writes it.
    Written(String),
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
    match (&drill.answer, given) {
        // Notation is compared as notation. The check and mate marks are
        // accepted either way — writing Nf3 for Nf3+ is knowing the move and
        // not yet the habit, and the answer shown afterwards carries the mark
        // so the habit arrives.
        (Answer::Written(want), Answer::Written(got)) => {
            let bare = |text: &str| text.trim().trim_end_matches(['+', '#']).to_owned();
            want.trim() == got.trim() || bare(want) == bare(got)
        }
        (a, b) => a == b,
    }
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
    next_on(rung, seed, None)
}

/// Deal a question, on a real position where the rung wants one.
///
/// `sample` is a position out of the player's own corpus. Without it the hard
/// rungs fall back to something made up, which still works and still teaches
/// less: the whole point of asking "what is hanging" is that it is the
/// question you failed to ask in a real game.
pub fn next_on(rung: Rung, seed: u64, sample: Option<&Chess>) -> Drill {
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
        Rung::ReadNotation => {
            let position = sample.cloned().unwrap_or_default();
            let legal = position.legal_moves();
            let mv = legal
                .get(rng.below(legal.len().max(1) as u64) as usize)
                .copied()
                .unwrap_or_else(|| position.legal_moves()[0]);
            let written = SanPlus::from_move(position.clone(), mv).to_string();
            // Which piece it means, not where it lands. Landing squares are
            // coordinates, which is the previous skill; working out *which*
            // knight is the one notation actually asks of you.
            Drill {
                rung,
                prompt: format!("{written} — click the piece that moves."),
                board: position.board().clone(),
                shown_move: Some(written),
                answer: Answer::Squares(mv.from().into_iter().collect()),
            }
        }
        Rung::WriteNotation => {
            let position = sample.cloned().unwrap_or_default();
            let legal = position.legal_moves();
            let mv = legal
                .get(rng.below(legal.len().max(1) as u64) as usize)
                .copied()
                .unwrap_or_else(|| position.legal_moves()[0]);
            let written = SanPlus::from_move(position.clone(), mv).to_string();
            let from = mv.from().map(|f| f.to_string()).unwrap_or_default();
            Drill {
                rung,
                prompt: format!("{from} to {} — write it.", mv.to()),
                board: position.board().clone(),
                shown_move: mv.from().map(|f| format!("{f}{}", mv.to())),
                answer: Answer::Written(written),
            }
        }
        Rung::Hanging => {
            let position = sample.cloned().unwrap_or_default();
            let mover = position.turn();
            Drill {
                rung,
                prompt: format!(
                    "{} to play. Click every {} piece that is hanging.",
                    side_name(mover),
                    side_name(mover).to_lowercase()
                ),
                board: position.board().clone(),
                shown_move: None,
                answer: Answer::Squares(hanging(&position, mover)),
            }
        }
        Rung::Recall => {
            let position = sample.cloned().unwrap_or_default();
            let board = position.board().clone();
            // Ask about a piece that is actually on the board, and prefer one
            // there is only one of, so the answer is a single square.
            let mut candidates: Vec<(Square, Piece)> = Vec::new();
            for square in Square::ALL {
                if let Some(piece) = board.piece_at(square) {
                    if matches!(piece.role, Role::Queen | Role::King | Role::Rook) {
                        candidates.push((square, piece));
                    }
                }
            }
            candidates.sort_by_key(|(square, _)| *square as u32);
            let (square, piece) = candidates
                .get(rng.below(candidates.len().max(1) as u64) as usize)
                .copied()
                .unwrap_or((Square::E1, white(Role::King)));
            Drill {
                rung,
                prompt: format!(
                    "Look, then it goes. Click where the {} {} was.",
                    side_name(piece.color).to_lowercase(),
                    name(piece.role).to_lowercase()
                ),
                board,
                shown_move: None,
                answer: Answer::Squares(BTreeSet::from([square])),
            }
        }
    }
}

fn side_name(color: Color) -> &'static str {
    match color {
        Color::White => "White",
        Color::Black => "Black",
    }
}

/// Pieces of `side` that the other side attacks and `side` does not defend.
///
/// The question every instinct player forgets to ask, and the one that decides
/// most games below master level.
pub fn hanging(position: &Chess, side: Color) -> BTreeSet<Square> {
    let board = position.board();
    let occupied = board.occupied();
    let mut out = BTreeSet::new();
    for square in Square::ALL {
        let Some(piece) = board.piece_at(square) else {
            continue;
        };
        if piece.color != side || piece.role == Role::King {
            continue;
        }
        let attacked = !board.attacks_to(square, side.other(), occupied).is_empty();
        let defended = !board.attacks_to(square, side, occupied).is_empty();
        if attacked && !defended {
            out.insert(square);
        }
    }
    out
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
    ///
    /// The rungs that want a real position get their variety from the corpus
    /// rather than from the seed — asked without one they are the starting
    /// position every time, which is exactly what the caller must not do.
    #[test]
    fn the_ladder_does_not_ask_the_same_thing_every_time() {
        for rung in Rung::LADDER {
            if rung.wants_a_real_position() {
                continue;
            }
            let asked: BTreeSet<String> =
                (0..50u64).map(|seed| next(rung, seed).prompt).collect();
            assert!(asked.len() > 5, "{rung:?} only asked {} things", asked.len());
        }
    }

    /// And with positions supplied, they do vary — otherwise handing them a
    /// corpus would change nothing.
    #[test]
    fn a_real_position_is_what_makes_the_hard_rungs_vary() {
        use shakmaty::fen::Fen;
        use shakmaty::CastlingMode;
        let boards: Vec<Chess> = [
            "4k3/8/8/3n4/8/8/8/3RK3 b - - 0 1",
            "r1bqkbnr/pppp1ppp/2n5/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R b KQkq - 3 3",
            "8/5k2/8/8/3Q4/8/5K2/7r w - - 0 1",
        ]
        .iter()
        .map(|fen| {
            fen.parse::<Fen>()
                .unwrap()
                .into_position::<Chess>(CastlingMode::Standard)
                .unwrap()
        })
        .collect();

        for rung in Rung::LADDER.iter().filter(|r| r.wants_a_real_position()) {
            let asked: BTreeSet<String> = boards
                .iter()
                .enumerate()
                .map(|(seed, board)| next_on(*rung, seed as u64, Some(board)).prompt)
                .collect();
            assert!(
                asked.len() > 1,
                "{rung:?} asked the same thing of three different positions"
            );
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

#[cfg(test)]
mod harder_rungs {
    use super::*;
    use shakmaty::fen::Fen;
    use shakmaty::CastlingMode;

    fn position(fen: &str) -> Chess {
        fen.parse::<Fen>()
            .expect("a legal fen")
            .into_position(CastlingMode::Standard)
            .expect("a legal position")
    }

    fn sq(name: &str) -> Square {
        name.parse().expect("a square")
    }

    /// Attacked and undefended, which is not the same as attacked. Getting
    /// this wrong would teach the player to count defended pieces as losses.
    #[test]
    fn hanging_means_attacked_and_not_defended() {
        // Black knight on d5 attacked by the white rook on d1. Nothing of
        // Black's defends it, so it hangs.
        let bare = position("4k3/8/8/3n4/8/8/8/3RK3 b - - 0 1");
        assert_eq!(
            hanging(&bare, Color::Black),
            BTreeSet::from([sq("d5")]),
            "an attacked, undefended knight"
        );

        // The same knight, now defended by a pawn on c6. Still attacked, no
        // longer hanging.
        let held = position("4k3/8/2p5/3n4/8/8/8/3RK3 b - - 0 1");
        assert!(
            hanging(&held, Color::Black).is_empty(),
            "a defended piece is not hanging: {:?}",
            hanging(&held, Color::Black)
        );

        // Nothing attacking it at all.
        let quiet = position("4k3/8/8/3n4/8/8/8/4K3 b - - 0 1");
        assert!(quiet_is_empty(&quiet), "an unattacked piece is not hanging");
    }

    fn quiet_is_empty(position: &Chess) -> bool {
        hanging(position, Color::Black).is_empty()
    }

    /// The king is never "hanging" — it cannot be taken, and reporting it
    /// would make every check look like a lost piece.
    #[test]
    fn the_king_is_never_hanging() {
        let checked = position("4k3/8/8/8/8/8/8/4RK2 b - - 0 1");
        assert!(
            hanging(&checked, Color::Black).is_empty(),
            "the king was reported as hanging"
        );
    }

    /// Recall asks about a piece that is actually there, and wants one square.
    #[test]
    fn recall_asks_about_a_piece_on_the_board() {
        let start = Chess::default();
        for seed in 0..100u64 {
            let drill = next_on(Rung::Recall, seed, Some(&start));
            let Answer::Squares(want) = &drill.answer else {
                panic!("recall should want a square");
            };
            assert_eq!(want.len(), 1, "one square, not {}", want.len());
            let square = *want.iter().next().unwrap();
            assert!(
                drill.board.piece_at(square).is_some(),
                "seed {seed}: asked about an empty square"
            );
        }
    }

    /// The ladder starts at the top. Somebody who already knows the board
    /// should not answer twenty "is f6 light" before anything happens.
    #[test]
    fn the_ladder_opens_at_the_hard_end() {
        assert_eq!(Rung::opening_rung(), Rung::Recall);
        assert_eq!(Rung::opening_rung().next(), None);
        assert_eq!(Rung::SquareColour.previous(), None);
        assert_eq!(Rung::FindSquare.previous(), Some(Rung::SquareColour));
    }

    /// Quicker to fall than to climb, and neither on a handful of answers.
    #[test]
    fn a_rung_that_is_too_hard_gives_way() {
        let quick = Duration::from_secs(1);
        let slow = Duration::from_secs(120);

        let mostly_wrong: Vec<_> = (0..FALL_AFTER).map(|_| (false, quick)).collect();
        assert!(should_step_down(&mostly_wrong, Rung::Recall));

        let right_but_crawling: Vec<_> = (0..FALL_AFTER).map(|_| (true, slow)).collect();
        assert!(
            should_step_down(&right_but_crawling, Rung::Recall),
            "two minutes an answer is too hard even when it is right"
        );

        let fine: Vec<_> = (0..FALL_AFTER).map(|_| (true, quick)).collect();
        assert!(!should_step_down(&fine, Rung::Recall));

        let too_few: Vec<_> = (0..FALL_AFTER - 1).map(|_| (false, quick)).collect();
        assert!(!should_step_down(&too_few, Rung::Recall), "two answers is not a verdict");

        // Falling must be quicker than climbing, or a rung one too high is a
        // wall rather than a step.
        const _: () = assert!(FALL_AFTER < NEEDED);
    }

    /// Both new rungs want a real position, and say so.
    #[test]
    fn the_hard_rungs_ask_for_a_real_position() {
        assert!(Rung::Hanging.wants_a_real_position());
        assert!(Rung::Recall.wants_a_real_position());
        assert!(!Rung::SquareColour.wants_a_real_position());
        assert!(!Rung::AfterOneMove.wants_a_real_position());
    }
}

#[cfg(test)]
mod notation_rungs {
    use super::*;
    use shakmaty::fen::Fen;
    use shakmaty::CastlingMode;

    fn position(fen: &str) -> Chess {
        fen.parse::<Fen>()
            .expect("a legal fen")
            .into_position(CastlingMode::Standard)
            .expect("a legal position")
    }

    /// Board vision teaches where squares are. It does not teach that a
    /// knight going to f3 is called Nf3, that a capture carries an x, that two
    /// knights need saying which, or that castling is O-O. Calculate mode asks
    /// for all of that, so the ladder has to cover it.
    #[test]
    fn reading_notation_asks_which_piece_moves_not_where_it_lands() {
        // Knights on b1 and f1 both reach d2, so the notation has to say
        // which. Knights on b1 and g1 share no square at all, which is what
        // the first version of this used.
        let both = position("4k3/8/8/8/8/8/8/1N1K1N2 w - - 0 1");
        for seed in 0..300u64 {
            let drill = next_on(Rung::ReadNotation, seed, Some(&both));
            let Answer::Squares(want) = &drill.answer else {
                panic!("reading notation should want the piece's square");
            };
            let square = *want.iter().next().expect("one square");
            assert!(
                both.board().piece_at(square).is_some(),
                "seed {seed}: pointed at an empty square"
            );
            assert!(
                drill.prompt.contains("piece that moves"),
                "seed {seed}: {}",
                drill.prompt
            );
        }
    }

    /// A disambiguated move does come up, or the rung never teaches the one
    /// part of notation people actually get wrong.
    #[test]
    fn reading_notation_includes_moves_that_must_say_which_piece() {
        let both = position("4k3/8/8/8/8/8/8/1N1K1N2 w - - 0 1");
        let asked: Vec<String> = (0..300u64)
            .map(|seed| next_on(Rung::ReadNotation, seed, Some(&both)).prompt)
            .collect();
        assert!(
            asked.iter().any(|p| p.starts_with("Nbd2") || p.starts_with("Nfd2")),
            "no disambiguated move was ever asked: {:?}",
            &asked[..5.min(asked.len())]
        );
    }

    /// Writing is the direction calculate mode needs, and it is graded as
    /// notation rather than as a square.
    #[test]
    fn writing_notation_is_graded_as_notation() {
        let start = Chess::default();
        let drill = next_on(Rung::WriteNotation, 7, Some(&start));
        let Answer::Written(want) = &drill.answer else {
            panic!("writing notation should want text");
        };
        assert!(judge(&drill, &Answer::Written(want.clone())));
        assert!(!judge(&drill, &Answer::Written("nonsense".to_owned())));
        // Whitespace either side is a typist, not a mistake.
        assert!(judge(&drill, &Answer::Written(format!("  {want}  "))));
    }

    /// Leaving off the check mark is knowing the move and not yet the habit.
    /// It counts, and the answer shown afterwards carries the mark.
    #[test]
    fn a_missing_check_mark_still_counts() {
        let checking = position("4k3/8/8/8/8/8/8/R3K3 w - - 0 1");
        let drill = Drill {
            rung: Rung::WriteNotation,
            prompt: String::new(),
            board: checking.board().clone(),
            shown_move: None,
            answer: Answer::Written("Ra8+".to_owned()),
        };
        assert!(judge(&drill, &Answer::Written("Ra8+".to_owned())));
        assert!(judge(&drill, &Answer::Written("Ra8".to_owned())));
        assert!(!judge(&drill, &Answer::Written("Rb8".to_owned())));
        // Case matters: files are lower, pieces upper, and "ra8" is not a move.
        assert!(!judge(&drill, &Answer::Written("ra8".to_owned())));
    }

    /// The notation rungs sit below the ones that need them and above the
    /// geometry, and both want a real position to ask about.
    #[test]
    fn notation_sits_between_geometry_and_the_hard_rungs() {
        let at = |rung: Rung| Rung::LADDER.iter().position(|r| *r == rung).unwrap();
        assert!(at(Rung::AfterOneMove) < at(Rung::ReadNotation));
        assert!(at(Rung::ReadNotation) < at(Rung::WriteNotation));
        assert!(Rung::ReadNotation.wants_a_real_position());
        assert!(Rung::WriteNotation.wants_a_real_position());
        assert!(Rung::WriteNotation.is_written());
        assert!(!Rung::ReadNotation.is_written(), "reading is clicked");
    }
}
