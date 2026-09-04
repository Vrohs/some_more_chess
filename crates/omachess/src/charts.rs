//! Charts for the progress view.
//!
//! Each one draws a series prepared and tested in the core, so what the picture
//! claims and what the numbers say cannot drift apart. Colours come from CSS so
//! both themes work, and every chart labels the values it actually reaches.

use std::cell::Cell;
use std::rc::Rc;

use gtk4::cairo::Context;
use gtk4::prelude::*;
use gtk4::{DrawingArea, EventControllerMotion};
use omachess_core::progress::{GamePoint, SlopePoint};

const PAD_LEFT: f64 = 46.0;
const PAD_RIGHT: f64 = 16.0;
const PAD_TOP: f64 = 16.0;
const PAD_BOTTOM: f64 = 30.0;

/// Pull a colour out of the widget's CSS so the drawing follows the theme.
fn ink(area: &DrawingArea) -> (f64, f64, f64) {
    let c = area.color();
    (c.red().into(), c.green().into(), c.blue().into())
}

fn set(cr: &Context, rgb: (f64, f64, f64), alpha: f64) {
    cr.set_source_rgba(rgb.0, rgb.1, rgb.2, alpha);
}

fn label(cr: &Context, x: f64, y: f64, text: &str, size: f64) {
    cr.set_font_size(size);
    cr.move_to(x, y);
    let _ = cr.show_text(text);
}

/// Each repeated puzzle as a line from its first solve to its latest.
///
/// This is the clearest statement the application can make: every line that
/// falls is the same puzzle solved faster by the same person.
pub fn slope_chart(points: Vec<SlopePoint>) -> DrawingArea {
    let area = DrawingArea::builder()
        .content_height(280)
        .hexpand(true)
        .build();
    area.add_css_class("omachess-chart");

    let hover: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));

    let motion = EventControllerMotion::new();
    {
        let hover = hover.clone();
        let area_ref = area.clone();
        let data = points.clone();
        motion.connect_motion(move |_, x, y| {
            let w = f64::from(area_ref.width());
            let h = f64::from(area_ref.height());
            let found = nearest_slope(&data, w, h, x, y);
            if hover.get() != found {
                hover.set(found);
                area_ref.queue_draw();
            }
        });
    }
    {
        let hover = hover.clone();
        let area_ref = area.clone();
        motion.connect_leave(move |_| {
            if hover.get().is_some() {
                hover.set(None);
                area_ref.queue_draw();
            }
        });
    }
    area.add_controller(motion);

    area.set_draw_func(move |area, cr, width, height| {
        let (w, h) = (f64::from(width), f64::from(height));
        let fg = ink(area);
        if points.is_empty() || w < 120.0 {
            set(cr, fg, 0.5);
            label(cr, PAD_LEFT, h / 2.0, "No puzzle has been solved twice yet.", 13.0);
            return;
        }

        let max = points
            .iter()
            .map(|p| p.first_seconds.max(p.latest_seconds))
            .fold(1.0, f64::max);
        let (x1, x2) = columns(w);

        // Horizontal guides, labelled with the seconds they stand for.
        set(cr, fg, 0.13);
        cr.set_line_width(1.0);
        for step in 0..=4 {
            let value = max * f64::from(step) / 4.0;
            let y = y_of(value, max, h);
            cr.move_to(PAD_LEFT, y);
            cr.line_to(w - PAD_RIGHT, y);
            let _ = cr.stroke();
        }
        set(cr, fg, 0.55);
        for step in 0..=4 {
            let value = max * f64::from(step) / 4.0;
            let y = y_of(value, max, h);
            label(cr, 6.0, y + 4.0, &format!("{value:.0}s"), 11.0);
        }

        // One line per puzzle.
        for (index, point) in points.iter().enumerate() {
            let focused = hover.get() == Some(index);
            let dim = hover.get().is_some() && !focused;
            let colour = if point.improved() { (0.31, 0.72, 0.47) } else { (0.85, 0.55, 0.22) };
            set(cr, colour, if dim { 0.12 } else if focused { 1.0 } else { 0.55 });
            cr.set_line_width(if focused { 3.0 } else { 1.6 });

            let ya = y_of(point.first_seconds, max, h);
            let yb = y_of(point.latest_seconds, max, h);
            cr.move_to(x1, ya);
            cr.line_to(x2, yb);
            let _ = cr.stroke();

            cr.arc(x1, ya, if focused { 4.0 } else { 2.5 }, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
            cr.arc(x2, yb, if focused { 4.0 } else { 2.5 }, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }

        // Column labels.
        set(cr, fg, 0.6);
        label(cr, x1 - 26.0, h - 10.0, "first solve", 11.0);
        label(cr, x2 - 28.0, h - 10.0, "latest solve", 11.0);

        // The hovered puzzle, named with its own numbers.
        if let Some(index) = hover.get() {
            if let Some(point) = points.get(index) {
                set(cr, fg, 0.95);
                label(
                    cr,
                    PAD_LEFT,
                    PAD_TOP,
                    &format!(
                        "{}  ·  band {}  ·  {:.1}s → {:.1}s  ·  {} solves",
                        point.puzzle_id,
                        point.band,
                        point.first_seconds,
                        point.latest_seconds,
                        point.solves
                    ),
                    12.0,
                );
            }
        }
    });
    area
}

fn columns(w: f64) -> (f64, f64) {
    let usable = w - PAD_LEFT - PAD_RIGHT;
    (PAD_LEFT + usable * 0.22, PAD_LEFT + usable * 0.78)
}

fn y_of(value: f64, max: f64, h: f64) -> f64 {
    let usable = h - PAD_TOP - PAD_BOTTOM;
    h - PAD_BOTTOM - (value / max) * usable
}

/// Which line the pointer is closest to, if any is close enough to mean it.
fn nearest_slope(points: &[SlopePoint], w: f64, h: f64, x: f64, y: f64) -> Option<usize> {
    if points.is_empty() {
        return None;
    }
    let max = points
        .iter()
        .map(|p| p.first_seconds.max(p.latest_seconds))
        .fold(1.0, f64::max);
    let (x1, x2) = columns(w);
    if x < x1 - 12.0 || x > x2 + 12.0 {
        return None;
    }
    let t = ((x - x1) / (x2 - x1)).clamp(0.0, 1.0);

    let mut best: Option<(usize, f64)> = None;
    for (index, point) in points.iter().enumerate() {
        let ya = y_of(point.first_seconds, max, h);
        let yb = y_of(point.latest_seconds, max, h);
        let distance = (ya + (yb - ya) * t - y).abs();
        if best.is_none_or(|(_, d)| distance < d) {
            best = Some((index, distance));
        }
    }
    best.filter(|(_, d)| *d < 14.0).map(|(i, _)| i)
}

/// A plain series over time, used for the rating trajectory and for game
/// accuracy. `band` optionally draws two reference levels.
pub fn line_chart(
    values: Vec<f64>,
    unit: &'static str,
    reference: Option<(f64, f64)>,
    good_is_up: bool,
) -> DrawingArea {
    let area = DrawingArea::builder()
        .content_height(200)
        .hexpand(true)
        .build();
    area.add_css_class("omachess-chart");

    let hover: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
    let motion = EventControllerMotion::new();
    {
        let hover = hover.clone();
        let area_ref = area.clone();
        let count = values.len();
        motion.connect_motion(move |_, x, _| {
            let w = f64::from(area_ref.width());
            let found = nearest_index(count, w, x);
            if hover.get() != found {
                hover.set(found);
                area_ref.queue_draw();
            }
        });
    }
    {
        let hover = hover.clone();
        let area_ref = area.clone();
        motion.connect_leave(move |_| {
            if hover.get().is_some() {
                hover.set(None);
                area_ref.queue_draw();
            }
        });
    }
    area.add_controller(motion);

    area.set_draw_func(move |area, cr, width, height| {
        let (w, h) = (f64::from(width), f64::from(height));
        let fg = ink(area);
        if values.len() < 2 || w < 120.0 {
            set(cr, fg, 0.5);
            label(cr, PAD_LEFT, h / 2.0, "Not enough history yet.", 13.0);
            return;
        }

        let hi = values.iter().cloned().fold(f64::MIN, f64::max);
        let lo = values.iter().cloned().fold(f64::MAX, f64::min);
        let span = if (hi - lo).abs() < 1e-9 { 1.0 } else { hi - lo };
        let top = hi + span * 0.12;
        let bottom = lo - span * 0.12;
        let scale = |v: f64| {
            let usable = h - PAD_TOP - PAD_BOTTOM;
            h - PAD_BOTTOM - ((v - bottom) / (top - bottom)) * usable
        };
        let x_at = |i: usize| {
            let usable = w - PAD_LEFT - PAD_RIGHT;
            PAD_LEFT + usable * i as f64 / (values.len() - 1) as f64
        };

        set(cr, fg, 0.13);
        cr.set_line_width(1.0);
        for step in 0..=3 {
            let value = bottom + (top - bottom) * f64::from(step) / 3.0;
            cr.move_to(PAD_LEFT, scale(value));
            cr.line_to(w - PAD_RIGHT, scale(value));
            let _ = cr.stroke();
        }
        set(cr, fg, 0.55);
        for step in 0..=3 {
            let value = bottom + (top - bottom) * f64::from(step) / 3.0;
            label(cr, 6.0, scale(value) + 4.0, &format!("{value:.0}"), 11.0);
        }

        if let Some((earlier, later)) = reference {
            for (value, alpha) in [(earlier, 0.35), (later, 0.6)] {
                set(cr, fg, alpha);
                cr.set_line_width(1.0);
                cr.set_dash(&[4.0, 4.0], 0.0);
                cr.move_to(PAD_LEFT, scale(value));
                cr.line_to(w - PAD_RIGHT, scale(value));
                let _ = cr.stroke();
                cr.set_dash(&[], 0.0);
            }
        }

        let improving = if good_is_up {
            values[values.len() - 1] >= values[0]
        } else {
            values[values.len() - 1] <= values[0]
        };
        let colour = if improving { (0.31, 0.72, 0.47) } else { (0.85, 0.55, 0.22) };

        set(cr, colour, 0.16);
        cr.move_to(x_at(0), h - PAD_BOTTOM);
        for (i, v) in values.iter().enumerate() {
            cr.line_to(x_at(i), scale(*v));
        }
        cr.line_to(x_at(values.len() - 1), h - PAD_BOTTOM);
        cr.close_path();
        let _ = cr.fill();

        set(cr, colour, 0.95);
        cr.set_line_width(2.0);
        cr.move_to(x_at(0), scale(values[0]));
        for (i, v) in values.iter().enumerate().skip(1) {
            cr.line_to(x_at(i), scale(*v));
        }
        let _ = cr.stroke();

        let last = values.len() - 1;
        cr.arc(x_at(last), scale(values[last]), 3.5, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();

        if let Some(index) = hover.get() {
            if let Some(v) = values.get(index) {
                set(cr, fg, 0.25);
                cr.set_line_width(1.0);
                cr.move_to(x_at(index), PAD_TOP);
                cr.line_to(x_at(index), h - PAD_BOTTOM);
                let _ = cr.stroke();
                set(cr, fg, 0.95);
                label(cr, PAD_LEFT, PAD_TOP, &format!("#{}  {v:.1}{unit}", index + 1), 12.0);
            }
        }
    });
    area
}

fn nearest_index(count: usize, w: f64, x: f64) -> Option<usize> {
    if count < 2 {
        return None;
    }
    let usable = w - PAD_LEFT - PAD_RIGHT;
    if usable <= 0.0 {
        return None;
    }
    let t = ((x - PAD_LEFT) / usable).clamp(0.0, 1.0);
    Some((t * (count - 1) as f64).round() as usize)
}

/// Accuracy values, oldest first.
pub fn accuracy_values(points: &[GamePoint]) -> Vec<f64> {
    points.iter().map(|p| p.accuracy).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pointer_outside_the_columns_selects_nothing() {
        let points = vec![SlopePoint {
            puzzle_id: "a".into(),
            band: 1100,
            first_seconds: 40.0,
            latest_seconds: 20.0,
            solves: 2,
        }];
        assert_eq!(nearest_slope(&points, 400.0, 280.0, 5.0, 100.0), None);
    }

    #[test]
    fn a_pointer_on_a_line_selects_it() {
        let points = vec![SlopePoint {
            puzzle_id: "a".into(),
            band: 1100,
            first_seconds: 40.0,
            latest_seconds: 40.0,
            solves: 2,
        }];
        let y = y_of(40.0, 40.0, 280.0);
        let (x1, _) = columns(400.0);
        assert_eq!(nearest_slope(&points, 400.0, 280.0, x1 + 1.0, y), Some(0));
    }

    #[test]
    fn indices_span_the_whole_width() {
        assert_eq!(nearest_index(5, 400.0, PAD_LEFT), Some(0));
        assert_eq!(nearest_index(5, 400.0, 400.0 - PAD_RIGHT), Some(4));
        assert_eq!(nearest_index(1, 400.0, 100.0), None);
    }

    #[test]
    fn the_scale_puts_larger_values_higher() {
        assert!(y_of(40.0, 40.0, 280.0) < y_of(10.0, 40.0, 280.0));
    }
}
