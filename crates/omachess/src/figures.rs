//! Stating a number so it reads as one.
//!
//! Both report panels in this application grew the same way: a figure was
//! computed, and then a sentence was written around it to explain what it
//! meant. Progress reached 877 words that way, wrapped around three results.
//! The reader came for a measurement and had to read an essay to find one.
//!
//! These are the four shapes a figure is allowed to take here. They live in one
//! place because the alternative — each panel growing its own — is how the same
//! label ended up written three different ways, and how prose comes back.

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Label, Orientation};

/// A headline number with its name above it, and an optional signed change.
pub fn tile(name: &str, value: &str, delta: Option<String>) -> GtkBox {
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

/// A row of tiles, which is what a handful of headline numbers wants to be
/// rather than a paragraph naming them one after another.
pub fn tiles(cells: Vec<GtkBox>) -> GtkBox {
    let row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(22)
        .build();
    for cell in cells {
        row.append(&cell);
    }
    row
}

/// One line of figures, monospaced with tabular digits so columns line up down
/// the page instead of drifting with the width of a 1.
pub fn stat_line(text: &str) -> Label {
    let label = Label::builder()
        .label(text)
        .halign(Align::Start)
        .wrap(true)
        .build();
    label.add_css_class("omachess-stat");
    label
}

pub fn section_title(text: &str) -> Label {
    let label = Label::builder().label(text).halign(Align::Start).build();
    label.add_css_class("title-4");
    label
}

/// At most a short line saying what a figure is measured against. Never an
/// explanation of what it means: that belongs in the manual, not beside every
/// number, and it is where the essay starts.
pub fn caption(text: &str) -> Label {
    let label = Label::builder()
        .label(text)
        .halign(Align::Start)
        .wrap(true)
        .max_width_chars(74)
        .build();
    label.add_css_class("dim-label");
    label
}
