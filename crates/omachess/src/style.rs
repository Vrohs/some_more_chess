//! Stylesheet loading.
//!
//! The built-in sheet is written entirely in libadwaita's named colours, which
//! Omarchy regenerates from the active theme's `colors.toml`. A user sheet at
//! `$XDG_CONFIG_HOME/omachess/omachess.css` is loaded afterwards so a generated
//! theme template can override the board without touching the binary.

use gtk4::{gdk, CssProvider};
use std::fs;
use std::path::PathBuf;

const DEFAULT_CSS: &str = r#"
.omachess-board {
    background-color: #6b4423;
    padding: 6px;
    border-radius: 4px;
    box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.45);
}
.omachess-square {
    padding: 0;
    /* Eight of these plus the study panel must fit a tiled window, so the
       floor is small; the board still grows to fill whatever space there is. */
    min-width: 26px;
    min-height: 26px;
    border-radius: 0;
}
/* The board is wood, not palette. Square colour is a property of a chess
   board, not of the desktop theme, and a scarlet or lilac board reads as a
   toy. Everything around the board still follows the Omarchy theme. */
/* Flat colour, no grain: a repeating gradient at this scale reads as diagonal
   streaks across the board rather than as timber, and it competes with the
   pieces for attention. */
.omachess-square.light { background-color: #e9cfa6; }
.omachess-square.dark  { background-color: #a9713f; }
/* The square the pointer went down on, so dragging feels like picking a piece
   up rather than nothing happening until release. */
/* The piece being carried, lifted slightly off the board. */
.omachess-ghost {
    opacity: 0.92;
}
.omachess-square.pressed {
    box-shadow: inset 0 0 0 3px rgba(255, 241, 138, 0.55);
}
/* The keyboard cursor: distinct from selection, since one is where you are
   looking and the other is what you have picked up. */
.omachess-square.cursor {
    box-shadow: inset 0 0 0 2px rgba(255, 255, 255, 0.75);
}
.omachess-square.selected {
    box-shadow: inset 0 0 0 3px rgba(255, 241, 138, 0.95);
}
.omachess-square.last-move {
    background-image: linear-gradient(rgba(255, 232, 120, 0.34), rgba(255, 232, 120, 0.34));
}
/* Mate is the end of the game, so it is unmistakable rather than a tint. */
.omachess-square.mated {
    background-image: radial-gradient(circle closest-side at center,
        rgba(214, 45, 40, 1.0) 0%,
        rgba(214, 45, 40, 0.75) 65%,
        rgba(214, 45, 40, 0.35) 100%);
    box-shadow: inset 0 0 0 4px rgba(255, 90, 80, 0.95);
}
.omachess-square.in-check {
    background-image: radial-gradient(circle closest-side at center,
        rgba(214, 45, 40, 0.90) 0%,
        rgba(214, 45, 40, 0.42) 60%,
        transparent 80%);
}
.omachess-piece {
    /* Pinned to fonts that carry the chess range as text glyphs. Without this
       the emoji font claims some of them and the pieces render inconsistently. */
    font-family: "DejaVu Sans", "Noto Sans Symbols 2", "FreeSerif", sans-serif;
    font-size: 34px;
}
/* Piece colour is game information, not decoration: a white piece must read as
   white in every theme. Only the squares follow the palette. The contrasting
   outline keeps both sides legible on light and dark squares alike. */
.omachess-piece.white-piece {
    color: #f4f4f4;
    text-shadow: 0 1px 2px rgba(0, 0, 0, 0.9), 0 0 1px rgba(0, 0, 0, 0.9);
}
.omachess-piece.black-piece {
    color: #121212;
    text-shadow: 0 1px 2px rgba(255, 255, 255, 0.55), 0 0 1px rgba(255, 255, 255, 0.7);
}
/* The board no longer answers a refused move by turning red: it said nothing
   about what was wrong, and it was the only feedback of its kind. Every board
   outcome now arrives as text across the window instead. */
.omachess-announce {
    font-size: 30px;
    font-weight: bold;
    padding: 18px 24px;
    margin: 0;
}
.omachess-announce.rejected { background-color: @error_color;   color: #fff; }
.omachess-announce.lost     { background-color: @error_color;   color: #fff; }
.omachess-announce.won      { background-color: @success_color; color: #fff; }
.omachess-announce.drawn    { background-color: @warning_color; color: #000; }
.omachess-announce.ended    { background-color: @accent_color;  color: #fff; }
.omachess-coord {
    font-size: 10px;
    font-weight: bold;
    opacity: 0.75;
}
.omachess-coord.on-light { color: #6b4423; }
.omachess-coord.on-dark  { color: #e9cfa6; }
.omachess-status { font-size: 15px; }
/* The promotion picker sits on the board itself. The column is opaque so the
   pieces underneath cannot be mistaken for part of the choice, and the rest of
   the board is dimmed so it reads as waiting for an answer. */
.omachess-promo-shade {
    background-color: rgba(0, 0, 0, 0.45);
}
.omachess-promo {
    background-color: #f2e4cd;
    border: 2px solid #6b4423;
    border-radius: 4px;
    box-shadow: 0 2px 12px rgba(0, 0, 0, 0.5);
}
.omachess-promo-choice {
    background: none;
    border: none;
    box-shadow: none;
    padding: 2px;
    min-width: 0;
    min-height: 0;
}
.omachess-promo-choice:hover {
    background-color: rgba(107, 68, 35, 0.25);
}
/* The glyph fallback has no art to scale, so it is sized to fill the square. */
.omachess-promo-choice .omachess-piece {
    font-size: 34px;
}
/* The clock is read at a glance under pressure, so it is large, monospaced so
   the digits do not shift as they count, and turns red once the move is being
   made on a low clock. */
.omachess-clock {
    font-size: 20px;
    font-family: monospace;
    font-feature-settings: "tnum";
}
.omachess-clock.error { color: @error_color; font-weight: bold; }
/* The sparkline reads its stroke colour from CSS, so the chart stays on the
   Omarchy palette even though the board no longer does. */
.omachess-spark { color: @accent_color; }
.omachess-spark.improving { color: @success_color; }
.omachess-spark.slowing { color: @warning_color; }
.omachess-change { font-weight: bold; }
.omachess-change.improving { color: @success_color; }
.omachess-change.slowing { color: @warning_color; }
.omachess-timer { font-family: monospace; font-size: 22px; }
"#;

pub fn install() {
    let Some(display) = gdk::Display::default() else {
        return;
    };

    let base = CssProvider::new();
    base.load_from_data(DEFAULT_CSS);
    gtk4::style_context_add_provider_for_display(
        &display,
        &base,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    for path in overrides() {
        if let Ok(css) = fs::read_to_string(&path) {
            let user = CssProvider::new();
            user.load_from_data(&css);
            gtk4::style_context_add_provider_for_display(
                &display,
                &user,
                gtk4::STYLE_PROVIDER_PRIORITY_USER,
            );
        }
    }
}

/// Stylesheets layered over the built-in one, in increasing precedence.
///
/// Omarchy renders `~/.config/omarchy/themed/*.tpl` into the active theme
/// directory, so dropping `omachess.css.tpl` there makes the board follow every
/// theme switch. An explicit file under the config directory wins over it.
fn overrides() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join(".local/state/omarchy/current/theme/omachess.css"));
    }
    if let Some(path) = user_stylesheet() {
        paths.push(path);
    }
    paths.retain(|p| p.exists());
    paths
}

fn user_stylesheet() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    let path = base.join("omachess/omachess.css");
    path.exists().then_some(path)
}
