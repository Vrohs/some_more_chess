//! What to do today.
//!
//! The application had grown very good at describing this player and useless at
//! directing them: six sections of findings, and the reader left to work out
//! what any of it meant for the next thirty minutes. Diagnosis without a
//! prescription is a report, not a trainer.
//!
//! So this reads everything already measured and returns a session — ordered by
//! how strong the evidence behind each piece is, with the number that justified
//! it attached, and stopping when the time is spent. Nothing here invents work:
//! a step only appears when the data that would justify it exists.

use anyhow::Result;

use crate::store::Store;

/// A session worth sitting down for. Long enough to be worth starting, short
/// enough to finish before the accuracy starts falling — the fatigue reading
/// is what suggested this rather than a round number.
pub const SESSION_MINUTES: u32 = 35;

/// No single kind of work may take more than this share of the session.
///
/// Without it the plan is first-come-first-served: fifteen due repeats fill
/// half the time and the weakness, opening and endgame the application went to
/// the trouble of diagnosing never appear. A session of only repeats is not a
/// plan, it is a queue.
const MAX_SHARE: u32 = SESSION_MINUTES / 3;

/// Rough minutes each kind of work takes, used only to fill the session
/// sensibly. Deliberately conservative: a plan that overruns gets abandoned.
const MINUTES_PER_REPEAT: u32 = 1;
const MINUTES_PER_DRILL: u32 = 5;
const MINUTES_PER_PUZZLE: u32 = 1;
const MINUTES_PER_GAME: u32 = 12;
const MINUTES_PER_ENDGAME: u32 = 6;

#[derive(Debug, Clone, PartialEq)]
pub enum Task {
    /// Puzzles the scheduler says are due. Time-sensitive: the whole point of
    /// spacing is that the interval matters.
    Repeats { due: u32 },
    /// Positions from games this player actually lost.
    Drill { positions: u32 },
    /// Puzzles on a theme they are measurably worse at than their own average.
    Weakness { theme: String, count: u32 },
    /// An opening they keep scoring badly in, played from the position it
    /// reaches.
    Opening { name: String, games: u32 },
    /// A theoretical endgame never yet converted.
    Endgame { name: &'static str },
    /// A game against the engine, when there is nothing more specific to do.
    Play,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub task: Task,
    pub minutes: u32,
    /// The measurement that put this in the plan, in the player's own numbers.
    /// A step that cannot say why it is here does not belong in a plan.
    pub why: String,
}

impl Step {
    pub fn headline(&self) -> String {
        match &self.task {
            Task::Repeats { due } => format!("Re-solve {due} due puzzle{}", plural(*due)),
            Task::Drill { positions } => {
                format!(
                    "Play out {positions} position{} you lost from",
                    plural(*positions)
                )
            }
            Task::Weakness { theme, count } => format!("{count} {theme} puzzles"),
            Task::Opening { name, .. } => format!("Play the {name}"),
            Task::Endgame { name } => format!("Convert: {name}"),
            Task::Play => "Play a game against the engine".to_owned(),
        }
    }
}

fn plural(n: u32) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Add a step if it fits both its share of the session and what is left.
fn add(steps: &mut Vec<Step>, spent: &mut u32, step: Step) -> bool {
    if step.minutes > MAX_SHARE || *spent + step.minutes > SESSION_MINUTES {
        return false;
    }
    *spent += step.minutes;
    steps.push(step);
    true
}

/// Build today's session.
///
/// Ordered by how directly the evidence bears on this player: what the
/// scheduler says is due first, because that is the only piece with a deadline;
/// then their own lost positions, which are the most specific evidence there
/// is; then a named weakness, an opening that keeps costing points, and an
/// endgame with a settled answer. A game only if nothing sharper is available.
pub fn todays_plan(store: &Store) -> Result<Vec<Step>> {
    let mut steps = Vec::new();
    let mut spent = 0;

    // 1. Due repeats. The interval is the measurement, so a late repeat is a
    //    weaker one — this is the only part of the plan with a deadline.
    let due = store.due_count(chrono::Utc::now())? as u32;
    if due > 0 {
        let count = due.min(MAX_SHARE / MINUTES_PER_REPEAT);
        add(
            &mut steps,
            &mut spent,
            Step {
                task: Task::Repeats { due: count },
                minutes: count * MINUTES_PER_REPEAT,
                why: format!(
                    "{due} are due. Re-solving late weakens the measurement, \
                     because the gap is the thing being measured."
                ),
            },
        );
    }

    // 2. Positions from lost games, worst first.
    let drills = store.drills_to_play(MAX_SHARE / MINUTES_PER_DRILL)?;
    if !drills.is_empty() {
        let worst = drills[0].1.lost * 100.0;
        let count = drills.len() as u32;
        add(
            &mut steps,
            &mut spent,
            Step {
                task: Task::Drill { positions: count },
                minutes: count * MINUTES_PER_DRILL,
                why: format!(
                    "The worst cost you {worst:.0}% of the result. These are your own \
                     games, not a stranger's puzzles."
                ),
            },
        );
    }

    // 3. A theme measurably below this player's own average.
    let (weaknesses, baseline) = crate::progress::recurring_weaknesses(store)?;
    if let Some(weakness) = weaknesses.first() {
        add(
            &mut steps,
            &mut spent,
            Step {
                task: Task::Weakness {
                    theme: weakness.theme.clone(),
                    count: MAX_SHARE / MINUTES_PER_PUZZLE,
                },
                minutes: MAX_SHARE,
                why: format!(
                    "You solve {:.0}% of these against {:.0}% overall, over {} attempts.",
                    weakness.success * 100.0,
                    baseline * 100.0,
                    weakness.attempts
                ),
            },
        );
    }

    // 4. An opening that keeps costing points.
    // A game cannot be cut into thirds, so this one is measured against the
    // session rather than the share.
    if let Some((record, _)) = crate::progress::openings_to_drill(store)?.first() {
        let step_minutes = MINUTES_PER_GAME;
        if spent + step_minutes <= SESSION_MINUTES {
            spent += step_minutes;
            steps.push(Step {
                task: Task::Opening {
                    name: record.name.clone(),
                    games: record.games,
                },
                minutes: step_minutes,
                why: format!(
                    "{}W {}D {}L over {} games — {:.0}% against a par of 50%.",
                    record.won,
                    record.drawn,
                    record.lost,
                    record.games,
                    record.score() * 100.0
                ),
            });
        }
    }

    // 5. An endgame never converted. Objective truth, and the cheapest place
    //    to find out something is missing.
    let converted = crate::progress::endgame_records(store)?;
    if let Some(entry) = crate::endgame::ENDGAMES
        .iter()
        .find(|e| !converted.iter().any(|r| r.key == e.key && r.achieved > 0))
    {
        let target = match entry.dtm {
            Some(dtm) => format!("Best play needs {} moves.", dtm.div_ceil(2)),
            None => "Best play holds it.".to_owned(),
        };
        add(
            &mut steps,
            &mut spent,
            Step {
                task: Task::Endgame { name: entry.name },
                minutes: MINUTES_PER_ENDGAME,
                why: format!(
                    "Never converted. {target} A tablebase settled the result, so this \
                     is the one measurement here with no opinion in it."
                ),
            },
        );
    }

    // 6. With no games on record, playing one is not a fallback: every other
    //    step here is built from game data, so without it the plan can only
    //    ever be generic.
    let games = store.games_mine()?.len();
    if games == 0 {
        let step = Step {
            task: Task::Play,
            minutes: MINUTES_PER_GAME,
            why: "No games on record. Everything else here is built from your own \
                  games, so one is worth more than any puzzle right now."
                .to_owned(),
        };
        // A game cannot be cut down to a share of the session, so it is
        // measured against what is left. If nothing is left it still goes in,
        // first: without a game the rest of the plan has nothing to work from.
        if spent + step.minutes <= SESSION_MINUTES {
            steps.push(step);
        } else {
            steps.insert(0, step);
        }
    }

    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{DrillOrigin, GameRecord, PhaseLoss};
    use chrono::{Duration, TimeZone, Utc};

    fn store_with_drills(n: u32) -> Store {
        let store = Store::in_memory().unwrap();
        store.set_setting("player_name", "vrohs").unwrap();
        let base = Utc.with_ymd_and_hms(2026, 9, 1, 9, 0, 0).unwrap();
        for i in 0..n {
            store
                .record_drill_origin(
                    &format!("d{i}"),
                    "https://lichess.org/g",
                    base,
                    40,
                    "Qh1",
                    "Qg3",
                    0.9 - f64::from(i) * 0.01,
                    "middlegame",
                    0.8,
                )
                .unwrap();
        }
        store
    }

    /// An empty database must still produce something to do, or the first
    /// launch is a dead end.
    /// A new player has no history, so the plan can only offer work that needs
    /// none — and must ask for the game that everything else is built from.
    #[test]
    fn a_new_player_gets_work_that_needs_no_history() {
        let store = Store::in_memory().unwrap();
        let plan = todays_plan(&store).unwrap();
        assert!(
            plan.iter().any(|s| s.task == Task::Play),
            "with no games on record, playing one is the prerequisite"
        );
        assert!(
            plan.iter().any(|s| matches!(s.task, Task::Endgame { .. })),
            "an endgame needs no history and is real work on day one"
        );
    }

    /// Lost positions are more specific evidence than any general weakness, so
    /// they come before it — but a deadline beats both.
    #[test]
    fn the_plan_is_ordered_by_how_directly_the_evidence_applies() {
        let store = store_with_drills(3);
        let plan = todays_plan(&store).unwrap();
        assert!(
            matches!(plan[0].task, Task::Drill { .. }),
            "own positions come first when nothing is due, got {:?}",
            plan[0].task
        );
    }

    /// Every step has to name the measurement that put it there. A plan that
    /// cannot say why is just a list of chores.
    ///
    /// The one exception is playing a first game, which is in the plan
    /// precisely because there is no measurement yet — it is the step that
    /// creates the evidence the others need.
    #[test]
    fn every_step_that_claims_a_measurement_shows_it() {
        let store = store_with_drills(2);
        for step in todays_plan(&store).unwrap() {
            assert!(!step.why.is_empty(), "{:?} has no reason", step.task);
            if step.task == Task::Play {
                continue;
            }
            assert!(
                step.why.chars().any(|c| c.is_ascii_digit()),
                "{:?} gives no number: {}",
                step.task,
                step.why
            );
        }
    }

    /// A plan that overruns gets abandoned, so it must fit the session.
    #[test]
    fn the_plan_fits_in_the_session() {
        let store = store_with_drills(40);
        let plan = todays_plan(&store).unwrap();
        let total: u32 = plan.iter().map(|s| s.minutes).sum();
        assert!(
            total <= SESSION_MINUTES,
            "planned {total} minutes against a {SESSION_MINUTES} minute session"
        );
        assert!(!plan.is_empty());
    }

    /// A step must never appear without the data behind it: no games means no
    /// opening to fix, and inventing one would be exactly the fluff this
    /// project exists to avoid.
    #[test]
    fn no_step_appears_without_the_evidence_for_it() {
        let store = store_with_drills(1);
        let plan = todays_plan(&store).unwrap();
        assert!(
            !plan.iter().any(|s| matches!(s.task, Task::Opening { .. })),
            "no games on record, so no opening can be named"
        );
        assert!(
            !plan.iter().any(|s| matches!(s.task, Task::Weakness { .. })),
            "no attempts, so no weakness can be named"
        );
    }

    /// Once games exist and one opening is scoring badly, it earns a place.
    #[test]
    fn an_opening_that_keeps_costing_points_reaches_the_plan() {
        let store = store_with_drills(1);
        let base = Utc.with_ymd_and_hms(2026, 9, 1, 9, 0, 0).unwrap();
        for (i, result) in ["lost", "lost", "lost", "won"].iter().enumerate() {
            store
                .record_game(&GameRecord {
                    played_at: base + Duration::days(i as i64),
                    player_white: true,
                    opponent_elo: 1320,
                    result: (*result).into(),
                    moves: 40,
                    accuracy: 80.0,
                    mean_loss: 0.05,
                    blunders: 1,
                    mistakes: 0,
                    inaccuracies: 0,
                    source: String::new(),
                    phases: [PhaseLoss::UNKNOWN; 3],
                    player: "vrohs".into(),
                    opening: "Caro-Kann Defense".into(),
                    book_plies: 4,
                    time_control: String::new(),
                    pressure_moves: 0,
                    pressure_blunders: 0,
                })
                .unwrap();
        }
        let plan = todays_plan(&store).unwrap();
        let opening = plan
            .iter()
            .find(|s| matches!(s.task, Task::Opening { .. }))
            .expect("a losing opening earns a place");
        assert!(opening.why.contains("25%"), "{}", opening.why);
    }

    #[allow(dead_code)]
    fn unused(_: DrillOrigin) {}
}
