//! Playing on from the position you got wrong.
//!
//! A puzzle asks for one move and then tells you whether you found it. That is
//! not what losing a game feels like, and it is not what fixing one requires:
//! the move is only the start, and the work is converting the position it
//! leaves you with, against an opponent still trying to beat you.
//!
//! So a drill hands back the position as it stood before the mistake, states
//! what the position was worth, and asks for the result. You play it out.

use shakmaty::Color;

use crate::endgame::Objective;
use crate::store::DrillOrigin;

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
    use chrono::{TimeZone, Utc};

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
