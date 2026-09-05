//! Asking which piece a pawn becomes.
//!
//! Defaulting to a queen is right almost always, and wrong exactly when it
//! matters: a knight that forks the king, or a rook that avoids stalemating an
//! opponent who has nothing else to move. A board that never asks cannot play
//! those positions at all, and a player who has just found one and watches the
//! application take a queen anyway has learned nothing except not to trust it.

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation, Popover, PositionType, Widget};
use shakmaty::Role;

/// The label for a promotion choice. Figurine rather than words, so the row
/// reads at a glance and needs no translating.
fn glyph(role: Role, white: bool) -> &'static str {
    match (role, white) {
        (Role::Queen, true) => "\u{2655}",
        (Role::Rook, true) => "\u{2656}",
        (Role::Bishop, true) => "\u{2657}",
        (Role::Knight, true) => "\u{2658}",
        (Role::Queen, false) => "\u{265B}",
        (Role::Rook, false) => "\u{265C}",
        (Role::Bishop, false) => "\u{265D}",
        (Role::Knight, false) => "\u{265E}",
        _ => "?",
    }
}

/// Offer `choices` next to `anchor`, calling `chosen` with the pick.
///
/// Nothing happens until a choice is made: dismissing the popover leaves the
/// pawn where it was rather than quietly queening it, because a move the player
/// did not choose is worse than no move at all.
pub fn ask(
    anchor: &impl IsA<Widget>,
    choices: &[Role],
    white: bool,
    chosen: impl Fn(Role) + 'static,
) {
    let popover = Popover::builder()
        .position(PositionType::Top)
        .autohide(true)
        .build();

    let row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(4)
        .build();

    let chosen = std::rc::Rc::new(chosen);
    for role in choices {
        let button = Button::builder()
            .label(glyph(*role, white))
            .css_classes(vec!["omachess-promotion".to_owned()])
            .build();
        let role = *role;
        let chosen = chosen.clone();
        let popover_ref = popover.clone();
        button.connect_clicked(move |_| {
            popover_ref.popdown();
            chosen(role);
        });
        row.append(&button);
    }

    let panel = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(4)
        .build();
    let heading = Label::builder()
        .label("Promote to")
        .halign(Align::Start)
        .build();
    heading.add_css_class("dim-label");
    panel.append(&heading);
    panel.append(&row);

    popover.set_child(Some(&panel));
    popover.set_parent(anchor.as_ref());
    // The popover holds the only reference to itself once shown; dropping it
    // with the parent set would leave a dangling child widget behind.
    let popover_ref = popover.clone();
    popover.connect_closed(move |_| popover_ref.unparent());
    popover.popup();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The glyphs have to differ by piece and by colour, or the row is four
    /// identical buttons and the choice is a guess.
    #[test]
    fn every_piece_has_its_own_symbol() {
        let roles = [Role::Queen, Role::Rook, Role::Bishop, Role::Knight];
        for white in [true, false] {
            let mut seen: Vec<&str> = roles.iter().map(|r| glyph(*r, white)).collect();
            let before = seen.len();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), before, "two pieces share a symbol");
        }
        assert_ne!(glyph(Role::Queen, true), glyph(Role::Queen, false));
    }

    /// A pawn is never a promotion target, and asking for one should not
    /// produce a button that means nothing.
    #[test]
    fn a_piece_that_cannot_be_promoted_to_is_marked_rather_than_guessed() {
        assert_eq!(glyph(Role::Pawn, true), "?");
        assert_eq!(glyph(Role::King, false), "?");
    }
}
