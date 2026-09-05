//! What the repeated puzzles actually show.
//!
//! Everything on this page is paired: each puzzle's latest correct solve
//! against its own first. Nothing here is derived from solving new material,
//! because a new puzzle has nothing to be compared against.

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Label, Orientation};
use omachess_core::progress::{
    EndgameRecord, GamePoint, Improvement, OpeningRecord, PlayTrend, PressureRecord, SlopePoint,
    Transfer, Weakness, MIN_GAMES, MIN_OPENING_GAMES, MIN_PRESSURE_MOVES, MIN_RATING_POINTS,
    MIN_THEME_ATTEMPTS, MIN_TRANSFER, SIGNIFICANT,
};
use omachess_core::store::MIN_REPEAT_HOURS;

use crate::charts;

/// Everything the page needs, gathered once.
pub struct ProgressData {
    pub weaknesses: Vec<Weakness>,
    pub baseline_success: f64,
    pub transfer: Vec<Transfer>,
    pub overall: Option<Improvement>,
    pub bands: Vec<(u32, Improvement)>,
    pub solved: u64,
    pub slopes: Vec<SlopePoint>,
    pub ratings: Vec<f64>,
    pub games: Vec<GamePoint>,
    pub play: Option<PlayTrend>,
    pub endgames: Vec<EndgameRecord>,
    pub openings: Vec<OpeningRecord>,
    pub pressure: Option<PressureRecord>,
    pub repeat_mode: bool,
}

pub struct ProgressView {
    root: GtkBox,
}

impl ProgressView {
    pub fn new() -> Self {
        let root = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(18)
            .margin_top(20)
            .margin_bottom(20)
            .margin_start(20)
            .margin_end(20)
            .build();
        Self { root }
    }

    pub fn widget(&self) -> &GtkBox {
        &self.root
    }

    pub fn refresh(&self, data: &ProgressData) {
        while let Some(child) = self.root.first_child() {
            self.root.remove(&child);
        }

        // The diagnosis leads: what keeps going wrong is more use than any
        // number, because it is the only thing here you can act on tomorrow.
        self.root.append(&section_title("What keeps costing you"));
        if data.weaknesses.is_empty() {
            self.root.append(&caption(&format!(
                "Nothing stands out yet. A theme needs {MIN_THEME_ATTEMPTS} attempts before a bad \
                 run can be told apart from a real weakness.",
            )));
        } else {
            self.root.append(&caption(
                "Themes you get wrong most often, worst first. These are drawn from what you \
                 actually missed, not from a syllabus.",
            ));
            for weakness in &data.weaknesses {
                self.root
                    .append(&weakness_row(weakness, data.baseline_success));
            }
        }

        // Transfer next, because it is the only figure that means "better at
        // chess" rather than "better at these puzzles".
        self.root
            .append(&section_title("On puzzles you have never seen"));
        self.root.append(&caption(
            "Solving a fresh puzzle faster cannot be remembering it. Rating band is held fixed, \
             so this compares puzzles of comparable difficulty rather than an easy run against \
             a hard one. This is the marker that means you have improved at chess.",
        ));
        if data.transfer.is_empty() {
            self.root.append(&caption(&format!(
                "Not enough yet — {MIN_TRANSFER} first encounters are needed within a single \
                 rating band before the earlier and later halves can be compared.",
            )));
        } else {
            for transfer in &data.transfer {
                self.root.append(&transfer_row(transfer));
            }
        }

        self.root
            .append(&section_title("On puzzles you had solved before"));
        self.root.append(&caption(&format!(
            "Retention, not skill: a puzzle re-solved quickly may simply be remembered. Only \
             repeats at least {MIN_REPEAT_HOURS:.0} hours apart are counted, because anything \
             sooner is recall of that position.",
        )));

        match data.overall.as_ref() {
            Some(overall) => {
                self.root.append(&headline(overall));
                self.root.append(&verdict(overall));
            }
            None => self
                .root
                .append(&empty_state(data.solved, data.repeat_mode)),
        }

        // The slope chart is the measurement itself, drawn: one line per
        // repeated puzzle, from its own first solve to its own latest.
        if !data.slopes.is_empty() {
            let improved = data.slopes.iter().filter(|p| p.improved()).count();
            self.root.append(&caption(&format!(
                "{improved} of {} repeated puzzles are faster than they were. Each line is one \
                 puzzle, compared only against itself — hover to name it.",
                data.slopes.len()
            )));
            self.root.append(&charts::slope_chart(data.slopes.clone()));
        }

        if data.bands.len() > 1 {
            self.root.append(&section_title("By rating band"));
            for (band, improvement) in &data.bands {
                self.root.append(&band_row(*band, improvement));
            }
        }

        self.root.append(&section_title("Blunders on a low clock"));
        match &data.pressure {
            None => self.root.append(&caption(&format!(
                "Needs {MIN_PRESSURE_MOVES} moves played with a low clock. Only games played here \
                 to a time control count — an imported game carries no clock, and counting it as \
                 comfortable play would flatten the very effect this looks for.",
            ))),
            Some(record) => {
                self.root.append(&caption(&format!(
                    "Across {} timed game{}: {:.1}% of your moves on a low clock were blunders, \
                     against {:.1}% with time in hand.",
                    record.games,
                    if record.games == 1 { "" } else { "s" },
                    record.pressure_rate() * 100.0,
                    record.calm_rate() * 100.0,
                )));
                if let Some(multiplier) = record.multiplier() {
                    let verdict = Label::builder()
                        .label(if multiplier >= 1.5 {
                            format!(
                                "{multiplier:.1}x more often when the clock is low. This is where \
                                 your games are decided — practise moving before it gets here."
                            )
                        } else if multiplier <= 0.75 {
                            format!("{multiplier:.1}x — a low clock is not what costs you.")
                        } else {
                            "About the same either way; the clock is not the cause.".to_owned()
                        })
                        .halign(Align::Start)
                        .wrap(true)
                        .build();
                    if multiplier >= 1.5 {
                        verdict.add_css_class("error");
                    }
                    self.root.append(&verdict);
                }
            }
        }

        self.root.append(&section_title("Openings"));
        if data.openings.is_empty() {
            self.root.append(&caption(&format!(
                "Nothing reached {MIN_OPENING_GAMES} times yet. Openings are recorded per game as \
                 they are played or imported, so this fills in as games accumulate.",
            )));
        } else {
            self.root.append(&caption(
                "What you actually play, worst score first. Book depth is how far you were still \
                 following a named line — leaving it early is not a fault in itself, but leaving \
                 it early and scoring badly is where preparation pays.",
            ));
            for record in &data.openings {
                self.root.append(&opening_row(record));
            }
        }

        self.root.append(&section_title("Endgames converted"));
        if data.endgames.is_empty() {
            self.root.append(&caption(
                "Nothing attempted yet. These are the only positions here with a settled answer: \
                 a tablebase says the result, so converting one is evidence that does not depend \
                 on an opponent, a rating pool or a search depth.",
            ));
        } else {
            self.root.append(&caption(
                "A tablebase settled each of these before it was offered, and the engine defends \
                 uncapped. Nothing else on this page is this hard to argue with.",
            ));
            for record in &data.endgames {
                self.root.append(&endgame_row(record));
            }
        }

        self.root
            .append(&section_title("What these numbers are not"));
        self.root.append(&caption(
            "Three different numbers here look like ratings and none of them are on the same \
             scale. The puzzle rating below is a difficulty level borrowed from Lichess's puzzle \
             pool. The engine rating further down is how strong an opponent you are holding. \
             Neither is your Lichess or Chess.com rating, and those two are not each other \
             either — the same player typically reads two to four hundred points lower on \
             Chess.com than on Lichess, because they are separate pools with separate formulas. \
             Compare each number only against its own history.",
        ));

        self.root.append(&section_title("Puzzle rating"));
        if data.ratings.len() < MIN_RATING_POINTS {
            self.root.append(&caption(&format!(
                "{} of {MIN_RATING_POINTS} attempts. A rating line drawn from fewer than that \
                 is a handful of coin flips with a trend through it, so it is not drawn yet.",
                data.ratings.len()
            )));
        } else {
            self.root.append(&caption(
                "Replayed from every attempt you have made: solving harder puzzles raises it, \
                 missing easier ones lowers it. It shows the difficulty you can handle, not how \
                 fast you handle it.",
            ));
            self.root
                .append(&charts::line_chart(data.ratings.clone(), "", None, true));
        }

        self.root.append(&section_title("Games against the engine"));
        self.root
            .append(&play_section(data.play.as_ref(), data.games.len()));
        if data.games.len() >= 2 {
            self.root.append(&caption(
                "Accuracy per game, oldest first. Dashed lines mark the median of the earlier \
                 and later halves.",
            ));
            let reference = data
                .play
                .as_ref()
                .map(|t| (t.earlier_accuracy, t.recent_accuracy));
            self.root.append(&charts::line_chart(
                charts::accuracy_values(&data.games),
                "%",
                reference,
                true,
            ));
        }

        self.root.append(&method_note());
    }
}

/// One band's transfer result, stated with the evidence behind it.
fn transfer_row(transfer: &Transfer) -> GtkBox {
    let outer = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(2)
        .build();

    let improving = transfer.improvement() >= 0.0;
    let headline = Label::builder()
        .label(format!(
            "{}–{}   {:.0}% {}",
            transfer.band,
            transfer.band + 99,
            transfer.improvement().abs() * 100.0,
            if improving { "faster" } else { "slower" }
        ))
        .halign(Align::Start)
        .build();
    headline.add_css_class("title-2");
    headline.add_css_class("omachess-change");
    headline.add_css_class(if improving { "improving" } else { "slowing" });

    let detail = Label::builder()
        .label(format!(
            "{:.1}s → {:.1}s on {} unseen puzzles solved · accuracy {:.0}% → {:.0}% over {} seen\n{}",
            transfer.earlier_seconds,
            transfer.later_seconds,
            transfer.solved,
            transfer.earlier_accuracy * 100.0,
            transfer.later_accuracy * 100.0,
            transfer.seen,
            if transfer.is_speed_accuracy_tradeoff() {
                format!(
                    "Faster, but you are getting more of them wrong — speed bought by guessing, \
                     not earned. Slow down until accuracy recovers. (p = {:.3})",
                    transfer.p_value
                )
            } else if transfer.is_significant() {
                format!("Unlikely to be chance (p = {:.3}).", transfer.p_value)
            } else {
                format!("Not yet distinguishable from chance (p = {:.3}).", transfer.p_value)
            }
        ))
        .halign(Align::Start)
        .wrap(true)
        .max_width_chars(74)
        .build();
    detail.add_css_class("dim-label");

    outer.append(&headline);
    outer.append(&detail);
    outer
}

/// One recurring weakness, with the evidence beside it.
fn weakness_row(weakness: &Weakness, baseline: f64) -> Label {
    let label = Label::builder()
        .label(format!(
            "{:<16} {:>3.0}%  over {:>3} attempts   {:.0} points below your {:.0}% average",
            weakness.theme,
            weakness.success * 100.0,
            weakness.attempts,
            (baseline - weakness.success) * 100.0,
            baseline * 100.0
        ))
        .halign(Align::Start)
        .build();
    label.add_css_class("monospace");
    label
}

/// One opening's record.
fn opening_row(record: &OpeningRecord) -> GtkBox {
    let row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(10)
        .build();

    let name = Label::builder()
        .label(&record.name)
        .halign(Align::Start)
        .hexpand(true)
        .wrap(true)
        .build();
    row.append(&name);

    let book = Label::builder()
        .label(format!("book {:.0} plies", record.mean_book_plies))
        .halign(Align::End)
        .build();
    book.add_css_class("dim-label");
    row.append(&book);

    let tally = Label::builder()
        .label(format!(
            "{}/{}/{} · {:.0}%",
            record.won,
            record.drawn,
            record.lost,
            record.score() * 100.0
        ))
        .halign(Align::End)
        .build();
    // Half a point a game is par against an equal opponent.
    if record.score() < 0.4 {
        tally.add_css_class("error");
    } else if record.score() > 0.6 {
        tally.add_css_class("success");
    }
    row.append(&tally);
    row
}

/// One endgame's conversion record.
fn endgame_row(record: &EndgameRecord) -> GtkBox {
    let row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(10)
        .build();

    let name = Label::builder()
        .label(record.name)
        .halign(Align::Start)
        .hexpand(true)
        .wrap(true)
        .build();
    row.append(&name);

    let objective = Label::builder()
        .label(record.objective.label())
        .halign(Align::End)
        .build();
    objective.add_css_class("dim-label");
    row.append(&objective);

    // A win in twice the necessary moves is still a win, but it is not yet
    // technique — and the tablebase makes "necessary" a fact, not an opinion.
    let efficiency = match (record.best_conversion, record.optimal_moves) {
        (Some(taken), Some(optimal)) => format!("  best {taken} vs {optimal} needed"),
        _ => String::new(),
    };
    let score = Label::builder()
        .label(format!(
            "{} / {}{efficiency}",
            record.achieved, record.attempts
        ))
        .halign(Align::End)
        .build();
    // The most recent attempt is what the player is actually able to do now,
    // so it is coloured rather than the average.
    match record.last_achieved {
        Some(true) => score.add_css_class("success"),
        Some(false) => score.add_css_class("error"),
        None => {}
    }
    row.append(&score);
    row
}

fn section_title(text: &str) -> Label {
    let label = Label::builder().label(text).halign(Align::Start).build();
    label.add_css_class("title-4");
    label
}

fn caption(text: &str) -> Label {
    let label = Label::builder()
        .label(text)
        .halign(Align::Start)
        .wrap(true)
        .max_width_chars(74)
        .build();
    label.add_css_class("dim-label");
    label
}

fn empty_state(solved: u64, repeat_mode: bool) -> GtkBox {
    let outer = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(10)
        .build();

    let title = Label::builder().label("Nothing measured yet").build();
    title.add_css_class("title-2");
    title.set_halign(Align::Start);

    let body = Label::builder()
        .label(format!(
            "You have solved {solved} puzzle{}.\n\n\
             Solving new puzzles builds your repertoire, but it cannot measure \
             progress — a puzzle you have never seen has nothing to compare against.\n\n\
             {}",
            if solved == 1 { "" } else { "s" },
            if repeat_mode {
                "Repeat is on — keep going. Five repeated puzzles are needed before anything \
                 is claimed."
            } else {
                "Turn on Repeat in the Train tab. It serves back puzzles you have already \
                 solved, and each one is then timed against your own first solve of it."
            }
        ))
        .justify(gtk4::Justification::Left)
        .wrap(true)
        .max_width_chars(58)
        .build();
    body.add_css_class("dim-label");

    outer.append(&title);
    outer.append(&body);
    outer
}

fn headline(result: &Improvement) -> GtkBox {
    let outer = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(4)
        .build();

    let improving = result.median_speedup >= 1.0;
    let percent = ((result.median_speedup - 1.0) * 100.0).abs();

    let value = Label::builder()
        .label(format!(
            "{percent:.0}% {}",
            if improving { "faster" } else { "slower" }
        ))
        .halign(Align::Start)
        .build();
    value.add_css_class("title-1");
    value.add_css_class(if improving { "improving" } else { "slowing" });
    value.add_css_class("omachess-change");

    let basis = Label::builder()
        .label(format!(
            "median across {} puzzle{} you had already solved · {:.1}s → {:.1}s",
            result.puzzles,
            if result.puzzles == 1 { "" } else { "s" },
            seconds(result.median_first),
            seconds(result.median_latest),
        ))
        .halign(Align::Start)
        .build();
    basis.add_css_class("dim-label");

    outer.append(&value);
    outer.append(&basis);
    outer
}

fn verdict(result: &Improvement) -> Label {
    // The counts and the probability are stated plainly, because "faster" on
    // its own is a claim and this is the evidence for it.
    let text = format!(
        "{} faster, {} slower, {} unchanged.\n{}",
        result.faster,
        result.slower,
        result.unchanged,
        if result.is_significant() && result.median_speedup >= 1.0 {
            format!(
                "Unlikely to be chance (p = {:.3}, below the {SIGNIFICANT} threshold).",
                result.p_value
            )
        } else {
            format!(
                "Not yet distinguishable from chance (p = {:.3}). Repeat more puzzles.",
                result.p_value
            )
        }
    );
    let label = Label::builder()
        .label(text)
        .halign(Align::Start)
        .wrap(true)
        .build();
    label.add_css_class("dim-label");
    label
}

fn band_row(band: u32, result: &Improvement) -> Label {
    let improving = result.median_speedup >= 1.0;
    let percent = ((result.median_speedup - 1.0) * 100.0).abs();
    let label = Label::builder()
        .label(format!(
            "{band}–{}   {:.1}s → {:.1}s   {percent:.0}% {}   ({} puzzles, p = {:.3})",
            band + 99,
            seconds(result.median_first),
            seconds(result.median_latest),
            if improving { "faster" } else { "slower" },
            result.puzzles,
            result.p_value,
        ))
        .halign(Align::Start)
        .build();
    label.add_css_class("monospace");
    label
}

fn method_note() -> Label {
    let label = Label::builder()
        .label(
            "Each puzzle is compared only against itself, so puzzle difficulty \
             cannot masquerade as progress. Only correct solves are timed, and \
             the probability is a one-sided sign test over how many puzzles \
             improved.",
        )
        .halign(Align::Start)
        .wrap(true)
        .max_width_chars(70)
        .build();
    label.add_css_class("dim-label");
    label
}

fn seconds(span: chrono::Duration) -> f64 {
    span.num_milliseconds() as f64 / 1000.0
}

/// How the engine games have gone, which is a separate question from how the
/// puzzles have gone and is held to a weaker standard of evidence.
fn play_section(trend: Option<&PlayTrend>, games: usize) -> GtkBox {
    let outer = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(6)
        .build();

    let Some(trend) = trend else {
        outer.append(&caption(&format!(
            "{games} game{} played. {MIN_GAMES} are needed before the earlier and later halves \
             can be compared.",
            if games == 1 { "" } else { "s" }
        )));
        return outer;
    };

    let improving = trend.recent_accuracy >= trend.earlier_accuracy;
    let value = Label::builder()
        .label(format!(
            "Accuracy {:.1}% → {:.1}%",
            trend.earlier_accuracy, trend.recent_accuracy
        ))
        .halign(Align::Start)
        .build();
    value.add_css_class("title-2");
    value.add_css_class("omachess-change");
    value.add_css_class(if improving { "improving" } else { "slowing" });

    let detail = Label::builder()
        .label(format!(
            "blunders {:.1} → {:.1} per 100 moves over {} games\n{}",
            trend.earlier_blunders_per_100,
            trend.recent_blunders_per_100,
            trend.games,
            if trend.is_significant() {
                format!(
                    "Unlikely to be chance (Mann-Whitney p = {:.3}).",
                    trend.p_value
                )
            } else {
                format!(
                    "Not yet distinguishable from chance (p = {:.3}).",
                    trend.p_value
                )
            }
        ))
        .halign(Align::Start)
        .wrap(true)
        .max_width_chars(74)
        .build();
    detail.add_css_class("dim-label");

    outer.append(&value);
    outer.append(&detail);
    outer.append(&caption(
        "Weaker evidence than the puzzle figure above: games are not paired and no two are \
         alike. What keeps them comparable is that the opponent is pinned near your rating.",
    ));
    outer
}
