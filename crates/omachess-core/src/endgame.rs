//! Theoretical endgames, and whether the player converted them.
//!
//! Everything else in this application measures against a moving target: a
//! puzzle rating is a crowd's opinion, an engine evaluation is a search result,
//! a Lichess rating is a pool. Endgames are the one part of chess with settled
//! truth — a position is a win or it is not, and a tablebase says which.
//!
//! So this is the least bubble-like thing the trainer can ask of you: here is a
//! position that is objectively won, convert it against an engine playing the
//! best defence there is. You either did or you did not.
//!
//! Every position below was adjudicated against the Lichess Syzygy tablebase
//! rather than taken from memory, and the distance-to-mate is recorded so a
//! claim can be re-checked. Three candidates were thrown out during that pass:
//! two were illegal, and one was a draw that had been written down as a win.

use shakmaty::fen::Fen;
use shakmaty::{CastlingMode, Chess, Color, Position};

/// What the player has to achieve, from White's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Objective {
    /// The position is won. Anything short of a win is a failure.
    Win,
    /// The position is level and the defence has to hold it.
    Draw,
}

impl Objective {
    pub fn label(self) -> &'static str {
        match self {
            Objective::Win => "Win it",
            Objective::Draw => "Hold the draw",
        }
    }
}

/// Whether an attempt met its objective.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Achieved,
    Failed,
    /// Still in progress: no result yet.
    Open,
}

/// One theoretical endgame to convert.
#[derive(Debug, Clone, Copy)]
pub struct Endgame {
    /// Stable identifier, used as the key results are recorded under.
    pub key: &'static str,
    pub name: &'static str,
    pub fen: &'static str,
    pub objective: Objective,
    /// What the position teaches, in one line.
    pub idea: &'static str,
    /// Distance to mate reported by the tablebase when this was verified, in
    /// plies. `None` for a drawn position, which has no mate to be distant from.
    pub dtm: Option<u32>,
}

impl Endgame {
    /// The starting position, which is known to parse because every entry is
    /// checked by the tests.
    pub fn position(&self) -> Option<Chess> {
        self.fen
            .parse::<Fen>()
            .ok()?
            .into_position(CastlingMode::Standard)
            .ok()
    }

    /// Judge a finished game. `winner` is `None` for a draw.
    pub fn judge(&self, winner: Option<Color>) -> Outcome {
        match (self.objective, winner) {
            (Objective::Win, Some(Color::White)) => Outcome::Achieved,
            (Objective::Win, _) => Outcome::Failed,
            // Winning a position that was only level beats the objective
            // rather than missing it.
            (Objective::Draw, None) | (Objective::Draw, Some(Color::White)) => Outcome::Achieved,
            (Objective::Draw, Some(Color::Black)) => Outcome::Failed,
        }
    }
}

/// The starting set. The player is White in every one, so the board never has
/// to be flipped mid-session and "your move" always means the same thing.
pub const ENDGAMES: &[Endgame] = &[
    Endgame {
        key: "kp-in-front",
        name: "King and pawn: king in front",
        fen: "3k4/8/3K4/3P4/8/8/8/8 w - - 0 1",
        objective: Objective::Win,
        idea: "With the king a rank ahead of the pawn the win is forced. Take \
               the opposition before pushing, not after.",
        dtm: Some(21),
    },
    Endgame {
        key: "kp-drawn",
        name: "King and pawn: pawn too far back",
        fen: "8/8/8/8/3k4/8/3P4/3K4 w - - 0 1",
        objective: Objective::Draw,
        idea: "The same material, one tempo short, is only a draw. Knowing \
               which side of that line you are on decides the endgame.",
        dtm: None,
    },
    Endgame {
        key: "kp-defence",
        name: "A pawn down: holding with the king",
        fen: "8/8/8/1p1k4/8/1K6/8/8 w - - 0 1",
        objective: Objective::Draw,
        idea: "A pawn down and still level. Stay in front of the pawn and take \
               the opposition; step aside once and it is lost.",
        dtm: None,
    },
    Endgame {
        key: "lucena",
        name: "Lucena: building the bridge",
        fen: "2K5/2P1k3/8/8/3R4/8/r7/8 w - - 0 1",
        objective: Objective::Win,
        idea: "The most important winning position in rook endgames. The rook \
               goes to the fourth rank to shield the king from checks.",
        dtm: Some(35),
    },
    Endgame {
        key: "rook-vs-pawn",
        name: "Rook against a pawn on the seventh",
        fen: "8/8/8/8/8/1k6/1p6/1K1R4 w - - 0 1",
        objective: Objective::Win,
        idea: "The pawn is one square from queening and it still loses. Cut the \
               king off and the pawn falls.",
        dtm: Some(27),
    },
    Endgame {
        key: "queen-vs-centre-pawn",
        name: "Queen against a central pawn",
        fen: "8/8/7K/Q7/8/8/3p4/3k4 w - - 0 1",
        objective: Objective::Win,
        idea: "Check, force the king in front of its own pawn, then walk your \
               king closer. Repeat until the pawn drops.",
        dtm: Some(29),
    },
    Endgame {
        key: "queen-vs-rook-pawn",
        name: "Queen against a rook pawn",
        fen: "8/8/7K/1Q6/8/8/p7/k7 w - - 0 1",
        objective: Objective::Draw,
        idea: "The same technique fails here: forcing the king in front of the \
               pawn is stalemate. A queen ahead and it is still only a draw.",
        dtm: None,
    },
    Endgame {
        key: "philidor",
        name: "Philidor: the drawing method",
        fen: "8/8/8/8/4pk2/8/r7/4K1R1 w - - 0 1",
        objective: Objective::Draw,
        idea: "The other half of rook endings. Hold the third rank until the \
               pawn advances, then check from behind and never stop.",
        dtm: None,
    },
    Endgame {
        key: "wrong-bishop",
        name: "The wrong bishop",
        fen: "7k/8/5K2/7P/8/8/8/5B2 w - - 0 1",
        objective: Objective::Draw,
        idea: "A bishop and a rook pawn, and it is still a draw: the bishop \
               does not control the queening square, so the king simply sits \
               in the corner. Knowing this decides whether to trade into it.",
        dtm: None,
    },
    Endgame {
        key: "opposite-bishops",
        name: "Opposite bishops: two pawns is not enough",
        fen: "8/8/4kb2/8/2P1P3/5B2/8/4K3 w - - 0 1",
        objective: Objective::Draw,
        idea: "Two extra pawns and no win, because the bishops never meet. \
               The most common drawn endgame that looks winning.",
        dtm: None,
    },
    Endgame {
        key: "two-bishops-mate",
        name: "Mating with two bishops",
        fen: "8/8/8/4k3/8/8/8/2B1KB2 w - - 0 1",
        objective: Objective::Win,
        idea: "Easier than bishop and knight but still a technique: the \
               bishops build a wall and the king drives along it.",
        dtm: Some(33),
    },
    Endgame {
        key: "queen-vs-rook",
        name: "Queen against rook",
        fen: "8/8/8/4k3/4r3/8/8/3QK3 w - - 0 1",
        objective: Objective::Win,
        idea: "Won, and genuinely hard: the rook has to be separated from its \
               king by zugzwang. Sixty-five moves at best play, so the \
               fifty-move rule is a real opponent here.",
        dtm: Some(65),
    },
    Endgame {
        key: "rook-mate",
        name: "Mating with a rook",
        fen: "8/8/8/4k3/8/8/8/R3K3 w - - 0 1",
        objective: Objective::Win,
        idea: "The box. Shrink it a rank at a time and never give a stalemate.",
        dtm: Some(27),
    },
    Endgame {
        key: "queen-mate",
        name: "Mating with a queen",
        fen: "8/8/8/3k4/8/8/8/Q3K3 w - - 0 1",
        objective: Objective::Win,
        idea: "Fastest of the basic mates, and the easiest to stalemate. Use \
               the knight's-move squeeze and bring the king.",
        dtm: Some(15),
    },
    Endgame {
        key: "bishop-knight-mate",
        name: "Mating with bishop and knight",
        fen: "8/8/8/4k3/8/8/8/1NB1K3 w - - 0 1",
        objective: Objective::Win,
        idea: "The hardest basic mate: the king must be driven to a corner the \
               bishop controls. Fifty moves is enough, but only just.",
        dtm: Some(59),
    },
];

/// How a position has finished: `Some(Some(colour))` for a win, `Some(None)`
/// for a draw, `None` while it is still going.
///
/// Shakmaty settles mate, stalemate and insufficient material, but not the
/// fifty-move rule — which in an endgame is not a detail. Half of these
/// positions are held by reaching it, and bishop and knight has to beat it.
pub fn conclusion(position: &Chess) -> Option<Option<Color>> {
    if position.is_checkmate() {
        return Some(Some(!position.turn()));
    }
    if position.is_stalemate() || position.is_insufficient_material() {
        return Some(None);
    }
    if position.halfmoves() >= 100 {
        return Some(None);
    }
    None
}

/// Moves left before the fifty-move rule ends it, which the defender is
/// counting down and the attacker is racing.
pub fn moves_until_fifty(position: &Chess) -> u32 {
    100u32.saturating_sub(position.halfmoves()).div_ceil(2)
}

/// Look one up by key.
pub fn find(key: &str) -> Option<&'static Endgame> {
    ENDGAMES.iter().find(|e| e.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_position_is_legal_and_white_to_move() {
        for endgame in ENDGAMES {
            let position = endgame
                .position()
                .unwrap_or_else(|| panic!("{} has an unusable FEN", endgame.key));
            assert_eq!(
                position.turn(),
                Color::White,
                "{}: the player is always White",
                endgame.key
            );
            assert!(
                !position.is_game_over(),
                "{}: there would be nothing to play",
                endgame.key
            );
            // Tablebase adjudication only covers seven pieces or fewer, so a
            // larger position would be an unverifiable claim.
            let pieces = position.board().occupied().count();
            assert!(
                pieces <= 7,
                "{}: {pieces} pieces is beyond the tablebase",
                endgame.key
            );
        }
    }

    #[test]
    fn keys_are_unique() {
        let mut keys: Vec<_> = ENDGAMES.iter().map(|e| e.key).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "endgame keys must be unique");
    }

    #[test]
    fn a_recorded_distance_to_mate_means_a_win_and_a_draw_has_none() {
        for endgame in ENDGAMES {
            match endgame.objective {
                Objective::Win => assert!(
                    endgame.dtm.is_some(),
                    "{}: a won position was verified with a distance to mate",
                    endgame.key
                ),
                Objective::Draw => assert!(
                    endgame.dtm.is_none(),
                    "{}: a drawn position has no mate to be distant from",
                    endgame.key
                ),
            }
        }
    }

    fn position_of(fen: &str) -> Chess {
        fen.parse::<Fen>()
            .unwrap()
            .into_position(CastlingMode::Standard)
            .unwrap()
    }

    #[test]
    fn the_fifty_move_rule_ends_an_endgame() {
        // Bishop and knight needs most of the fifty, so the count has to be
        // right or the trainer would call a win a draw.
        let nearly = position_of("8/8/8/4k3/8/8/8/1NB1K3 w - - 98 60");
        assert_eq!(conclusion(&nearly), None, "one move still remains");
        assert_eq!(moves_until_fifty(&nearly), 1);

        let expired = position_of("8/8/8/4k3/8/8/8/1NB1K3 w - - 100 61");
        assert_eq!(conclusion(&expired), Some(None), "the draw is reached");
        assert_eq!(moves_until_fifty(&expired), 0);
    }

    #[test]
    fn mate_and_stalemate_are_told_apart() {
        // Black is mated, so White won.
        let mated = position_of("7k/6Q1/6K1/8/8/8/8/8 b - - 0 1");
        assert_eq!(conclusion(&mated), Some(Some(Color::White)));

        // The same idea one file over is stalemate, which fails a won position.
        let stalemate = position_of("7k/5Q2/5K2/8/8/8/8/8 b - - 0 1");
        assert_eq!(conclusion(&stalemate), Some(None));

        let playing = position_of(find("lucena").unwrap().fen);
        assert_eq!(conclusion(&playing), None);
    }

    #[test]
    fn judging_follows_the_objective() {
        let win = find("lucena").unwrap();
        assert_eq!(win.judge(Some(Color::White)), Outcome::Achieved);
        assert_eq!(
            win.judge(None),
            Outcome::Failed,
            "a draw fails a won position"
        );
        assert_eq!(win.judge(Some(Color::Black)), Outcome::Failed);

        let hold = find("kp-drawn").unwrap();
        assert_eq!(hold.judge(None), Outcome::Achieved);
        assert_eq!(hold.judge(Some(Color::Black)), Outcome::Failed);
        // Beating the objective is not missing it.
        assert_eq!(hold.judge(Some(Color::White)), Outcome::Achieved);
    }
}
