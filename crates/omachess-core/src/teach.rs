//! Saying why a move was wrong.
//!
//! "Not the move — try again" is a flashcard. It tells the solver they failed
//! and nothing about the position, so the only thing available to learn is the
//! answer by rote, which is the one thing the measurement here is designed to
//! see through.
//!
//! What teaches is the refutation: the move the opponent answers with, and
//! where it leaves you. That is a question about a position nobody has
//! analysed — the position after a mistake the puzzle's author never
//! considered — so it needs an engine, and it is the one place in the solving
//! loop where an engine belongs.
//!
//! It belongs there because of when it happens. A wrong move sets the attempt's
//! `correct` flag to false, and every measurement in this application counts
//! only correct attempts: `paired_solves` filters `WHERE correct = 1`, and
//! `transfer_by_band` drops anything else before it times a thing. So from the
//! moment a wrong move is played, that attempt's clock has already left the
//! measurement, and the engine can take as long as it needs without touching a
//! single figure. Asking one move earlier would corrupt every number here.

use shakmaty::Chess;

use crate::engine::{Analysis, Score};

/// What punishes a wrong move, and where it leaves the solver.
#[derive(Debug, Clone, PartialEq)]
pub struct Refutation {
    /// The opponent's answer, written the way a player writes it.
    pub reply: Option<String>,
    /// The solver's chances after that answer, in `0.0..=1.0`.
    pub solver_chance: f64,
    /// Moves until the solver is mated, when that is the answer.
    pub mated_in: Option<u32>,
}

/// Read an engine's verdict on the position a wrong move led to.
///
/// `after` is that position, so the side to move is the opponent and every
/// figure the engine reports is from *their* point of view. Forgetting that
/// inverts the advice, which is worse than giving none.
pub fn refutation(after: &Chess, analysis: &Analysis) -> Option<Refutation> {
    let score = analysis.score?;
    let opponent_chance = score.win_chance();
    let reply = analysis
        .pv
        .first()
        .or(analysis.best_move.as_ref())
        .and_then(|uci| san_of(after, uci));
    let mated_in = match score {
        // Positive mate here is the opponent mating, which is the solver being
        // mated. A mate for the solver is not a refutation of anything.
        Score::Mate(n) if n > 0 => Some(n as u32),
        _ => None,
    };
    Some(Refutation {
        reply,
        solver_chance: 1.0 - opponent_chance,
        mated_in,
    })
}

impl Refutation {
    /// One line, stating what answers the move and what it costs.
    ///
    /// Deliberately without adjectives. "A careless move" tells the solver
    /// about themselves; "after Rxb4 you are lost" tells them about the board.
    pub fn describe(&self, played: &str) -> String {
        let answer = match &self.reply {
            Some(reply) => format!("{played} — after {reply}"),
            None => format!("{played} —"),
        };
        match self.mated_in {
            Some(1) => format!("{answer} you are mated."),
            Some(n) => format!("{answer} you are mated in {n}."),
            None => format!(
                "{answer} your chances are {:.0}%.",
                self.solver_chance * 100.0
            ),
        }
    }
}

/// One move in the notation a player reads, or nothing if it will not play.
fn san_of(position: &Chess, uci: &str) -> Option<String> {
    crate::game::line_to_san(position, std::slice::from_ref(&uci.to_owned()))
        .into_iter()
        .next()
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

    /// The engine speaks for the side to move, and after a wrong move that is
    /// the opponent. Reporting their number as the solver's would tell someone
    /// who has just blundered that they are winning.
    #[test]
    fn the_engine_speaks_for_the_opponent_and_is_read_as_such() {
        // Black to move and completely winning: a huge score here is a
        // disaster for the solver, who is White.
        let after = position("6k1/5ppp/8/8/8/6K1/5PPP/1r6 b - - 0 1");
        let analysis = Analysis {
            best_move: Some("b1b8".to_owned()),
            score: Some(Score::Cp(900)),
            depth: 20,
            pv: vec!["b1b8".to_owned()],
        };
        let found = refutation(&after, &analysis).expect("a verdict");
        assert!(
            found.solver_chance < 0.05,
            "the solver was told they had {:.2} after being crushed",
            found.solver_chance
        );
        assert_eq!(found.reply.as_deref(), Some("Rb8"));
    }

    /// A mate for the side to move after the blunder is a mate against the
    /// solver, and has to be said that way round.
    #[test]
    fn a_mate_for_the_opponent_is_stated_as_being_mated() {
        let after = position("6k1/5ppp/8/8/8/6K1/5PPP/1r6 b - - 0 1");
        let analysis = Analysis {
            best_move: Some("b1b8".to_owned()),
            score: Some(Score::Mate(2)),
            depth: 20,
            pv: vec!["b1b8".to_owned()],
        };
        let found = refutation(&after, &analysis).expect("a verdict");
        assert_eq!(found.mated_in, Some(2));
        let said = found.describe("Rb4");
        assert!(said.contains("mated in 2"), "{said}");
        assert!(said.contains("Rb4"), "{said}");
    }

    /// A mate the solver is delivering refutes nothing, and must never be read
    /// as one.
    #[test]
    fn a_mate_the_solver_is_giving_is_not_a_refutation() {
        let after = position("6k1/5ppp/8/8/8/6K1/5PPP/1r6 b - - 0 1");
        let analysis = Analysis {
            best_move: None,
            score: Some(Score::Mate(-3)),
            depth: 20,
            pv: Vec::new(),
        };
        let found = refutation(&after, &analysis).expect("a verdict");
        assert_eq!(found.mated_in, None);
        assert!(found.solver_chance > 0.95);
    }

    /// An engine that returned nothing usable must produce no advice at all
    /// rather than confident advice about a score it never gave.
    #[test]
    fn no_score_means_no_claim() {
        let after = position("6k1/5ppp/8/8/8/6K1/5PPP/1r6 b - - 0 1");
        assert_eq!(refutation(&after, &Analysis::default()), None);
    }

    /// The reply has to be legal in the position it is claimed for. A line that
    /// cannot be played is worse than silence: it reads as analysis.
    #[test]
    fn an_unplayable_answer_is_dropped_rather_than_shown() {
        let after = position("6k1/5ppp/8/8/8/6K1/5PPP/1r6 b - - 0 1");
        let analysis = Analysis {
            best_move: Some("a1a8".to_owned()),
            score: Some(Score::Cp(50)),
            depth: 12,
            pv: vec!["a1a8".to_owned()],
        };
        let found = refutation(&after, &analysis).expect("a verdict");
        assert_eq!(found.reply, None);
        assert!(found.describe("Rb4").contains("Rb4"));
    }

}
