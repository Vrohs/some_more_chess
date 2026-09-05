//! Playing on from the position you got wrong.
//!
//! A puzzle asks for one move and then tells you whether you found it. That is
//! not what losing a game feels like, and it is not what fixing one requires:
//! the move is only the start, and the work is converting the position it
//! leaves you with, against an opponent still trying to beat you.
//!
//! So a drill hands back the position as it stood before the mistake, states
//! what the position was worth, and asks for the result. You play it out.

use chrono::{DateTime, Utc};
use shakmaty::Color;

use crate::endgame::Objective;
use crate::store::DrillOrigin;

/// How many successful play-outs retire a position.
///
/// Not one. A single success cannot tell skill apart from the engine having a
/// bad day, and a position retired on one lucky win is a hole in the training
/// that nothing will ever point at again.
pub const RETIRE_SUCCESSES: usize = 2;

/// Whether a position has been mastered well enough to stop offering it.
///
/// "Satisfying" is not a feeling here, it is whatever rules out the three ways
/// a success can be hollow:
///
/// - **Luck.** One win is a sample of one, so two are required.
/// - **Recall.** Winning a position again twenty minutes later is remembering
///   the moves, not playing them. The successes have to be at least
///   [`MIN_REPEAT_HOURS`](crate::store::MIN_REPEAT_HOURS) apart — the same
///   threshold the puzzle measurement already uses, for the same reason.
/// - **Regression.** Two wins last month and a loss yesterday is not mastery.
///   The most recent attempt has to be one of the successes.
///
/// A position that fails any of these comes straight back into the pool, so
/// the rule repairs itself: lose a retired position once and it returns.
pub fn is_retired(attempts: &[(DateTime<Utc>, bool)]) -> bool {
    // Regression: whatever happened last is what the player can do now.
    if !attempts.last().map(|(_, ok)| *ok).unwrap_or(false) {
        return false;
    }
    let wins: Vec<DateTime<Utc>> = attempts
        .iter()
        .filter(|(_, ok)| *ok)
        .map(|(at, _)| *at)
        .collect();
    if wins.len() < RETIRE_SUCCESSES {
        return false;
    }
    // Recall: the gap that matters is between the earliest and latest success,
    // since anything narrower is the same sitting rehearsing itself.
    let span = *wins.last().expect("checked non-empty") - wins[0];
    span.num_minutes() as f64 / 60.0 >= crate::store::MIN_REPEAT_HOURS
}

/// The player was clearly better than this before it counted as winning.
const WINNING: f64 = 0.60;
/// Below this they were already worse, and saving it is the honest ask.
const LOSING: f64 = 0.40;

/// What a drill asks for, given what the position was worth.
///
/// Demanding a win from a position that was already lost teaches nothing but
/// that the trainer is not paying attention, so the objective follows the
/// evaluation rather than always being "win".
pub fn objective_for(win_before: f64) -> Objective {
    if win_before >= WINNING {
        Objective::Win
    } else {
        // Level or worse: holding it is the result that was actually available.
        Objective::Draw
    }
}

/// How the drill puts the ask to the player.
pub fn brief(origin: &DrillOrigin) -> String {
    let moves = origin.ply / 2 + 1;
    let when = origin.played_at.format("%-d %b");
    if origin.win_before < 0.0 {
        return format!(
            "Your game on {when}, move {moves}. You played {} here and it cost {:.0}% \
             of the result. Play it again.",
            origin.played,
            origin.lost * 100.0
        );
    }
    let standing = if origin.win_before >= WINNING {
        format!("You were winning ({:.0}%)", origin.win_before * 100.0)
    } else if origin.win_before < LOSING {
        format!("You were worse ({:.0}%)", origin.win_before * 100.0)
    } else {
        format!("It was level ({:.0}%)", origin.win_before * 100.0)
    };
    format!(
        "Your game on {when}, move {moves}. {standing}, then {} cost {:.0}%. \
         Play it out and get the result.",
        origin.played,
        origin.lost * 100.0
    )
}

/// Whether a played-out drill met its objective. `winner` is `None` for a draw.
pub fn judge(objective: Objective, player: Color, winner: Option<Color>) -> bool {
    match objective {
        Objective::Win => winner == Some(player),
        // Saving a lost or level position means not losing it.
        Objective::Draw => winner != Some(!player),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn origin(win_before: f64, lost: f64) -> DrillOrigin {
        DrillOrigin {
            source: "https://lichess.org/x".into(),
            played_at: Utc.with_ymd_and_hms(2026, 8, 21, 12, 0, 0).unwrap(),
            ply: 40,
            played: "Qh1".into(),
            best: "Qg3".into(),
            lost,
            phase: "middlegame".into(),
            win_before,
        }
    }

    /// Asking for a win from a position that was already lost teaches nothing
    /// except that the trainer is not reading the position.
    fn at(hours: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 1, 9, 0, 0).unwrap() + chrono::Duration::hours(hours)
    }

    /// One win is a sample of one, and a position retired on it is a hole in
    /// the training that nothing will ever point at again.
    #[test]
    fn a_single_success_does_not_retire_a_position() {
        assert!(!is_retired(&[(at(0), true)]));
    }

    /// Winning it again the same evening is remembering the moves.
    #[test]
    fn two_successes_in_one_sitting_are_recall_not_mastery() {
        assert!(!is_retired(&[(at(0), true), (at(2), true)]));
        // Far enough apart and it counts.
        assert!(is_retired(&[(at(0), true), (at(21), true)]));
    }

    /// Two wins last month and a loss yesterday is not mastery, and the
    /// position has to come back.
    #[test]
    fn a_recent_loss_brings_a_retired_position_back() {
        let mastered = [(at(0), true), (at(30), true)];
        assert!(is_retired(&mastered));

        let then_lost = [(at(0), true), (at(30), true), (at(60), false)];
        assert!(
            !is_retired(&then_lost),
            "losing it again puts it back in the pool"
        );

        // Winning it once more, far enough from the first success, retires it
        // again — the rule repairs itself rather than needing a reset.
        let recovered = [
            (at(0), true),
            (at(30), true),
            (at(60), false),
            (at(90), true),
        ];
        assert!(is_retired(&recovered));
    }

    #[test]
    fn failures_alone_never_retire_anything() {
        assert!(!is_retired(&[]));
        assert!(!is_retired(&[
            (at(0), false),
            (at(30), false),
            (at(60), false)
        ]));
    }

    #[test]
    fn the_objective_follows_what_was_actually_available() {
        assert_eq!(objective_for(0.85), Objective::Win);
        assert_eq!(objective_for(0.60), Objective::Win);
        assert_eq!(objective_for(0.50), Objective::Draw, "level: hold it");
        assert_eq!(objective_for(0.15), Objective::Draw, "worse: save it");
    }

    #[test]
    fn winning_is_required_only_where_the_win_was_there() {
        let white = Color::White;
        assert!(judge(Objective::Win, white, Some(white)));
        assert!(
            !judge(Objective::Win, white, None),
            "a draw fails a won game"
        );
        assert!(!judge(Objective::Win, white, Some(Color::Black)));
    }

    /// Saving a position means not losing it, so winning one you only had to
    /// hold is a pass, not a miss.
    #[test]
    fn saving_a_position_means_not_losing_it() {
        let white = Color::White;
        assert!(judge(Objective::Draw, white, None));
        assert!(
            judge(Objective::Draw, white, Some(white)),
            "better than asked"
        );
        assert!(!judge(Objective::Draw, white, Some(Color::Black)));
    }

    #[test]
    fn the_brief_says_where_it_came_from_and_what_was_at_stake() {
        let text = brief(&origin(0.82, 0.55));
        assert!(text.contains("21 Aug"), "{text}");
        assert!(text.contains("move 21"), "{text}");
        assert!(text.contains("winning"), "{text}");
        assert!(text.contains("Qh1"), "{text}");

        assert!(brief(&origin(0.45, 0.30)).contains("level"));
        assert!(brief(&origin(0.20, 0.15)).contains("worse"));
    }

    /// Positions recorded before the evaluation was kept must still read
    /// sensibly rather than claiming the player was losing at -100%.
    #[test]
    fn an_unrecorded_evaluation_does_not_invent_one() {
        let text = brief(&origin(-1.0, 0.4));
        assert!(!text.contains("-100"), "{text}");
        assert!(text.contains("Play it again"), "{text}");
    }
}
