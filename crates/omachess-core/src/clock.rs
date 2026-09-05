//! Playing to a clock.
//!
//! An untimed game trains a skill nobody uses in a rated game. This player's
//! own record says their losses come from single moves rather than from playing
//! badly throughout, and single moves are what a falling clock produces — so
//! how much time was left when a move was played is not decoration, it is the
//! measurement the rest of the application has been missing.

use std::time::Duration;

use shakmaty::{Color, Position};

/// Base time plus the increment added after each move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeControl {
    pub base: Duration,
    pub increment: Duration,
}

impl TimeControl {
    pub const fn new(base_secs: u64, increment_secs: u64) -> Self {
        Self {
            base: Duration::from_secs(base_secs),
            increment: Duration::from_secs(increment_secs),
        }
    }

    /// The usual "10+5" shorthand.
    pub fn label(&self) -> String {
        format!("{}+{}", self.base.as_secs() / 60, self.increment.as_secs())
    }

    /// Below this much remaining, a move counts as made under pressure.
    ///
    /// A fixed number of seconds would mean different things at different time
    /// controls, so it is a share of the base time, floored so that a very
    /// short game still has a meaningful window.
    pub fn pressure_threshold(&self) -> Duration {
        let share = self.base / 5;
        share.max(Duration::from_secs(15))
    }
}

/// The presets offered, rapid first: this player's real games are rapid.
pub const PRESETS: &[(&str, TimeControl)] = &[
    ("Rapid 10+0", TimeControl::new(600, 0)),
    ("Rapid 15+10", TimeControl::new(900, 10)),
    ("Blitz 5+0", TimeControl::new(300, 0)),
    ("Blitz 3+2", TimeControl::new(180, 2)),
    ("Classical 30+20", TimeControl::new(1800, 20)),
];

/// How a game ended on time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    /// The side that ran out lost.
    Lost(Color),
    /// The side that ran out is not lost: the opponent cannot mate with what
    /// they have left, which is a draw rather than a win.
    DrawnByInsufficientMaterial(Color),
}

/// Two counting-down clocks.
#[derive(Debug, Clone)]
pub struct Clock {
    control: TimeControl,
    /// Indexed white, black.
    remaining: [Duration; 2],
    flagged: Option<Color>,
}

fn index(side: Color) -> usize {
    match side {
        Color::White => 0,
        Color::Black => 1,
    }
}

impl Clock {
    pub fn new(control: TimeControl) -> Self {
        Self {
            control,
            remaining: [control.base; 2],
            flagged: None,
        }
    }

    pub fn control(&self) -> TimeControl {
        self.control
    }

    pub fn remaining(&self, side: Color) -> Duration {
        self.remaining[index(side)]
    }

    pub fn flagged(&self) -> Option<Color> {
        self.flagged
    }

    /// What the clock reads mid-turn, with `thinking` already spent but not yet
    /// committed. Display only: the move has not been made.
    pub fn showing(&self, side: Color, thinking: Duration) -> Duration {
        self.remaining(side).saturating_sub(thinking)
    }

    /// Whether a move played now, after `thinking`, is made under pressure.
    pub fn under_pressure(&self, side: Color, thinking: Duration) -> bool {
        self.showing(side, thinking) < self.control.pressure_threshold()
    }

    /// Charge a completed move and add the increment.
    ///
    /// Returns false when that move used the last of the time, which ends the
    /// game — the increment is not paid to a clock that has already fallen.
    pub fn commit(&mut self, side: Color, thinking: Duration) -> bool {
        if self.flagged.is_some() {
            return false;
        }
        let left = self.remaining[index(side)];
        if thinking >= left {
            self.remaining[index(side)] = Duration::ZERO;
            self.flagged = Some(side);
            return false;
        }
        self.remaining[index(side)] = left - thinking + self.control.increment;
        true
    }

    /// Called while a side is still thinking, to end the game the moment the
    /// time is gone rather than waiting for a move that will never come.
    pub fn expire(&mut self, side: Color, thinking: Duration) -> bool {
        if self.flagged.is_some() {
            return true;
        }
        if thinking >= self.remaining[index(side)] {
            self.remaining[index(side)] = Duration::ZERO;
            self.flagged = Some(side);
            return true;
        }
        false
    }
}

/// What running out of time means in `position`.
///
/// Losing on time is not automatic: if the opponent could not deliver mate with
/// the material they hold, the game is drawn.
pub fn flag_outcome(position: &impl Position, ran_out: Color) -> Flag {
    if position.has_insufficient_material(!ran_out) {
        Flag::DrawnByInsufficientMaterial(ran_out)
    } else {
        Flag::Lost(ran_out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shakmaty::fen::Fen;
    use shakmaty::{CastlingMode, Chess};

    fn position(fen: &str) -> Chess {
        fen.parse::<Fen>()
            .unwrap()
            .into_position(CastlingMode::Standard)
            .unwrap()
    }

    #[test]
    fn time_is_charged_and_the_increment_paid() {
        let mut clock = Clock::new(TimeControl::new(600, 5));
        assert!(clock.commit(Color::White, Duration::from_secs(30)));
        // Thirty spent, five back.
        assert_eq!(clock.remaining(Color::White), Duration::from_secs(575));
        assert_eq!(clock.remaining(Color::Black), Duration::from_secs(600));
        assert!(clock.flagged().is_none());
    }

    #[test]
    fn the_increment_cannot_rescue_a_clock_that_has_fallen() {
        let mut clock = Clock::new(TimeControl::new(60, 30));
        assert!(!clock.commit(Color::White, Duration::from_secs(61)));
        assert_eq!(clock.remaining(Color::White), Duration::ZERO);
        assert_eq!(clock.flagged(), Some(Color::White));
    }

    #[test]
    fn time_runs_out_mid_thought_without_waiting_for_a_move() {
        let mut clock = Clock::new(TimeControl::new(30, 0));
        assert!(!clock.expire(Color::Black, Duration::from_secs(29)));
        assert!(clock.expire(Color::Black, Duration::from_secs(30)));
        assert_eq!(clock.flagged(), Some(Color::Black));
    }

    #[test]
    fn pressure_is_a_share_of_the_time_control_not_a_fixed_count() {
        // A fifth of the base time in each case.
        assert_eq!(
            TimeControl::new(600, 0).pressure_threshold(),
            Duration::from_secs(120)
        );
        assert_eq!(
            TimeControl::new(1800, 20).pressure_threshold(),
            Duration::from_secs(360)
        );
        // Floored, so a very short game still has a usable window.
        assert_eq!(
            TimeControl::new(60, 0).pressure_threshold(),
            Duration::from_secs(15)
        );
    }

    #[test]
    fn a_move_counts_as_pressured_only_once_the_clock_is_low() {
        let clock = Clock::new(TimeControl::new(600, 0));
        assert!(!clock.under_pressure(Color::White, Duration::from_secs(400)));
        // 600 - 490 = 110, inside the 120-second window.
        assert!(clock.under_pressure(Color::White, Duration::from_secs(490)));
    }

    #[test]
    fn flagging_against_a_bare_king_is_a_draw_not_a_loss() {
        // White has only a king, so Black running out of time cannot lose.
        let bare = position("4k3/8/8/8/8/8/8/4K3 w - - 0 1");
        assert_eq!(
            flag_outcome(&bare, Color::Black),
            Flag::DrawnByInsufficientMaterial(Color::Black)
        );

        // With a queen on the board it is an ordinary loss on time.
        let armed = position("4k3/8/8/8/8/8/8/3QK3 w - - 0 1");
        assert_eq!(flag_outcome(&armed, Color::Black), Flag::Lost(Color::Black));
    }

    #[test]
    fn labels_read_the_way_players_say_them() {
        assert_eq!(TimeControl::new(600, 0).label(), "10+0");
        assert_eq!(TimeControl::new(180, 2).label(), "3+2");
    }
}
