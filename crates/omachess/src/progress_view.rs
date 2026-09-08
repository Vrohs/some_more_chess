//! What the repeated puzzles actually show.
//!
//! Everything on this page is paired: each puzzle's latest correct solve
//! against its own first. Nothing here is derived from solving new material,
//! because a new puzzle has nothing to be compared against.

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Label, Orientation};
use omachess_core::progress::{
    EndgameRecord, GamePoint, Improvement, OpeningRecord, PlayTrend, PressureRecord, SlopePoint,
    Transfer, MIN_GAMES, MIN_PAIRS, MIN_RATING_POINTS, MIN_THEME_ATTEMPTS, MIN_TRANSFER,
};

use crate::charts;

/// Everything the page needs, gathered once.
pub struct ProgressData {
    /// Every theme with enough attempts to be worth drawing, worst first.
    pub themes: Vec<(String, f64, u32)>,
    pub baseline_success: f64,
    pub transfer: Vec<Transfer>,
    pub overall: Option<Improvement>,
    pub solved: u64,
    pub slopes: Vec<SlopePoint>,
    pub ratings: Vec<f64>,
    pub games: Vec<GamePoint>,
    pub play: Option<PlayTrend>,
    pub endgames: Vec<EndgameRecord>,
    pub openings: Vec<OpeningRecord>,
    pub pressure: Option<PressureRecord>,
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

        // The verdict, the numbers, then the pictures. This page carried eight
        // hundred words of explanation around three real results; the reader
        // came for a measurement and had to read an essay to find one. What a
        // figure means belongs in the manual, not beside every figure.
        self.root.append(&hero(&data.transfer));
        self.root.append(&tiles(data));

        if data.ratings.len() >= MIN_RATING_POINTS {
            self.root.append(&section_title("Puzzle rating"));
            self.root.append(&caption("Lichess puzzle scale, not your own rating."));
            self.root
                .append(&charts::line_chart(data.ratings.clone(), "", None, true));
        }

        // Three is the fewest bars that show a spread; below that they are just
        // numbers wearing a chart.
        if data.themes.len() >= 3 {
            self.root.append(&section_title("Accuracy by theme"));
            self.root.append(&caption("Line marks your overall accuracy."));
            self.root.append(&charts::bar_chart(
                data.themes.clone(),
                data.baseline_success,
            ));
        }

        if !data.slopes.is_empty() {
            let improved = data.slopes.iter().filter(|p| p.improved()).count();
            self.root.append(&section_title("Repeats, each against itself"));
            self.root.append(&caption(&format!(
                "{improved} of {} faster than before.",
                data.slopes.len()
            )));
            self.root.append(&charts::slope_chart(data.slopes.clone()));
            if let Some(overall) = data.overall.as_ref() {
                self.root.append(&stat_line(&format!(
                    "{:.0}% faster   p {:.2}   n {}",
                    (overall.median_speedup - 1.0) * 100.0,
                    overall.p_value,
                    overall.puzzles
                )));
            }
        }

        let pending = pending_rows(data);
        if !pending.is_empty() {
            self.root.append(&section_title("Not measurable yet"));
            for row in pending {
                self.root.append(&stat_line(&row));
            }
        }

        // The tabs these belong to are hidden until they are finished, and a
        // report on a tab nobody can open is the same fluff in another place.
        if std::env::var_os("OMACHESS_ALL_TABS").is_some() {
            self.root.append(&section_title("Games against the engine"));
            self.root
                .append(&play_section(data.play.as_ref(), data.games.len()));
            if data.games.len() >= 2 {
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
            for record in &data.openings {
                self.root.append(&opening_row(record));
            }
            for record in &data.endgames {
                self.root.append(&endgame_row(record));
            }
            if let Some(record) = &data.pressure {
                self.root.append(&stat_line(&format!(
                    "low clock {:.1}%   time in hand {:.1}%   n {}",
                    record.pressure_rate() * 100.0,
                    record.calm_rate() * 100.0,
                    record.games
                )));
            }
        }
    }
}

/// The one figure the page leads with: are you solving unseen puzzles faster?
///
/// Unseen, because a puzzle you have met before can be remembered rather than
/// understood. Held to one rating band, because a run of easy puzzles is faster
/// than a run of hard ones whatever the solver does. It is the only number here
/// that means "better at chess" rather than "better at these puzzles".
fn hero(transfer: &[Transfer]) -> GtkBox {
    let outer = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(4)
        .build();
    outer.append(&section_title("Better at chess?"));

    // The band with the most first encounters leads: it is the one carrying the
    // most evidence, whichever way it points.
    let leading = transfer.iter().max_by_key(|t| t.seen);
    let Some(lead) = leading else {
        let figure = Label::builder().label("—").halign(Align::Start).build();
        figure.add_css_class("omachess-hero");
        outer.append(&figure);
        outer.append(&caption(&format!(
            "Needs {MIN_TRANSFER} unseen puzzles in one band."
        )));
        return outer;
    };

    // A result that could be chance is not a result. Saying so in the figure
    // rather than in a footnote is the difference between a measurement and a
    // number that flatters.
    let proven = transfer.iter().find(|t| t.is_significant());
    let (text, class) = match proven {
        Some(t) => (
            format!("{:.0}% {}", t.improvement().abs() * 100.0, direction(t)),
            if t.improvement() >= 0.0 {
                "improving"
            } else {
                "slowing"
            },
        ),
        None => ("Not proven yet".to_owned(), "dim-label"),
    };
    let figure = Label::builder().label(&text).halign(Align::Start).build();
    figure.add_css_class("omachess-hero");
    figure.add_css_class(class);
    outer.append(&figure);
    let _ = lead;

    // Ascending, so the bands read the way a player thinks of them.
    let mut ordered: Vec<&Transfer> = transfer.iter().collect();
    ordered.sort_by_key(|t| t.band);
    for t in ordered {
        outer.append(&stat_line(&format!(
            "{}–{}   {:.0}% {}   p {:.2}   n {}",
            t.band,
            t.band + 99,
            t.improvement().abs() * 100.0,
            direction(t),
            t.p_value,
            t.seen
        )));
    }
    outer
}

fn direction(t: &Transfer) -> &'static str {
    if t.improvement() >= 0.0 {
        "faster"
    } else {
        "slower"
    }
}

/// The headline numbers, side by side.
fn tiles(data: &ProgressData) -> GtkBox {
    let row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(28)
        .build();
    row.append(&tile("Solved", &data.solved.to_string(), None));
    row.append(&tile("Attempts", &data.ratings.len().to_string(), None));
    row.append(&tile(
        "Accuracy",
        &format!("{:.0}%", data.baseline_success * 100.0),
        None,
    ));
    let now = data.ratings.last().copied().unwrap_or(0.0);
    let first = data.ratings.first().copied().unwrap_or(0.0);
    let delta = now - first;
    row.append(&tile(
        "Rating",
        &format!("{now:.0}"),
        (data.ratings.len() >= 2).then(|| format!("{:+.0}", delta)),
    ));
    row
}

fn tile(name: &str, value: &str, delta: Option<String>) -> GtkBox {
    let cell = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(1)
        .build();
    let label = Label::builder().label(name).halign(Align::Start).build();
    label.add_css_class("dim-label");
    label.add_css_class("omachess-tile-label");
    let figure = Label::builder().label(value).halign(Align::Start).build();
    figure.add_css_class("omachess-tile");
    cell.append(&label);
    cell.append(&figure);
    if let Some(delta) = delta {
        let change = Label::builder().label(&delta).halign(Align::Start).build();
        change.add_css_class("omachess-tile-label");
        change.add_css_class(if delta.starts_with('-') {
            "slowing"
        } else {
            "improving"
        });
        cell.append(&change);
    }
    cell
}

/// One line of numbers, monospaced so columns line up down the page.
fn stat_line(text: &str) -> Label {
    let label = Label::builder().label(text).halign(Align::Start).build();
    label.add_css_class("omachess-stat");
    label
}

/// What is not measurable yet, as counters rather than paragraphs.
///
/// "Needs twelve first encounters within a single rating band before the
/// earlier and later halves can be compared" is a sentence. "4 / 12" is the
/// same fact, and it tells you how close you are.
fn pending_rows(data: &ProgressData) -> Vec<String> {
    let mut rows = Vec::new();
    if data.overall.is_none() {
        rows.push(format!(
            "Repeats           {} / {} pairs",
            data.slopes.len(),
            MIN_PAIRS
        ));
    }
    if data.ratings.len() < MIN_RATING_POINTS {
        rows.push(format!(
            "Rating trend      {} / {MIN_RATING_POINTS} attempts",
            data.ratings.len()
        ));
    }
    if data.themes.len() < 3 {
        rows.push(format!(
            "Themes            {} / 3 at {MIN_THEME_ATTEMPTS} attempts",
            data.themes.len()
        ));
    }
    if data.transfer.is_empty() {
        rows.push(format!("Unseen puzzles    0 / {MIN_TRANSFER} in one band"));
    }
    rows
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
    if let Some(class) = opening_class(record.score()) {
        tally.add_css_class(class);
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
    let efficiency = efficiency_text(record.best_conversion, record.optimal_moves);
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

/// Par in an opening is half a point a game. These are the distances from par
/// worth colouring rather than leaving to be read off the number.
const OPENING_POOR: f64 = 0.4;
const OPENING_GOOD: f64 = 0.6;


fn opening_class(score: f64) -> Option<&'static str> {
    if score < OPENING_POOR {
        Some("error")
    } else if score > OPENING_GOOD {
        Some("success")
    } else {
        None
    }
}

/// How a conversion compares to the fewest moves the position allows.
fn efficiency_text(best: Option<u32>, optimal: Option<u32>) -> String {
    match (best, optimal) {
        (Some(taken), Some(optimal)) => format!("  best {taken} vs {optimal} needed"),
        _ => String::new(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Par is half a point a game, and an opening at par should read as
    /// neither a success nor a failure.
    #[test]
    fn an_opening_at_par_is_left_uncoloured() {
        assert_eq!(opening_class(0.50), None, "par is not a verdict");
        assert_eq!(
            opening_class(OPENING_POOR),
            None,
            "the boundary is not poor"
        );
        assert_eq!(opening_class(OPENING_GOOD), None, "nor good");
        assert_eq!(opening_class(0.25), Some("error"));
        assert_eq!(opening_class(0.90), Some("success"));
    }

    /// A conversion is only worth comparing when both halves are known; a
    /// position with no distance to mate is drawn and has nothing to beat.
    #[test]
    fn efficiency_is_shown_only_when_there_is_something_to_compare() {
        assert_eq!(
            efficiency_text(Some(41), Some(18)),
            "  best 41 vs 18 needed"
        );
        assert_eq!(efficiency_text(None, Some(18)), "", "never converted");
        assert_eq!(
            efficiency_text(Some(41), None),
            "",
            "a drawn position has no target"
        );
        assert_eq!(efficiency_text(None, None), "");
    }
}
