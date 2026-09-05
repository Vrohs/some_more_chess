//! Saying what just happened, across the width of the window.
//!
//! The board used to answer a refused move by tinting itself red for a moment.
//! It said nothing about what was wrong, it was easy to miss if you were
//! looking at the piece rather than the board, and it was the only feedback of
//! its kind — checkmate, a draw, a lost game and a rejected move each arrived
//! by a different route, in a different place, at a different size.
//!
//! This is one mechanism for all of them: a line of text across the window,
//! stating the outcome in words. It never takes input, so it cannot swallow the
//! next click, and a rejection clears itself while a result stays until the
//! next position replaces it.

use std::cell::RefCell;
use std::time::Duration;

use gtk4::prelude::*;
use gtk4::{glib, Align, Label, Overlay};

/// What kind of thing is being announced. Only the colour differs; the
/// mechanism is deliberately identical for all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// The move was refused: illegal, not yours, not now.
    Rejected,
    /// The player got the result they were after.
    Won,
    /// The player did not.
    Lost,
    /// Neither, and that is sometimes the goal.
    Drawn,
    /// A statement of fact with no verdict attached.
    Ended,
}

impl Tone {
    fn css(self) -> &'static str {
        match self {
            Tone::Rejected => "rejected",
            Tone::Won => "won",
            Tone::Lost => "lost",
            Tone::Drawn => "drawn",
            Tone::Ended => "ended",
        }
    }

    /// Whether the message clears itself. A refused move is a moment; a result
    /// is a state, and it stays until the next position replaces it.
    fn is_transient(self) -> bool {
        self == Tone::Rejected
    }
}

/// How long a refused move stays on screen. Long enough to read six words,
/// short enough not to sit over the position while you look for the real move.
const TRANSIENT_MS: u64 = 1_800;

struct Banner {
    label: Label,
    /// The timer hiding the current transient message, cancelled whenever a
    /// new one replaces it so the second does not inherit the first's clock.
    hide: Option<glib::SourceId>,
    /// What was last said, for the self-test to read back.
    last: Option<(Tone, String)>,
}

thread_local! {
    /// One window, one main thread, one banner. A parameter threaded through
    /// four view constructors would say the same thing with more ceremony.
    static BANNER: RefCell<Option<Banner>> = const { RefCell::new(None) };
}

/// Put the banner over `overlay`, which should wrap the whole window content.
pub fn install(overlay: &Overlay) {
    let label = Label::builder()
        .halign(Align::Fill)
        .valign(Align::Start)
        .wrap(true)
        .justify(gtk4::Justification::Center)
        .visible(false)
        .build();
    label.add_css_class("omachess-announce");
    // The whole point is that it never eats the next move.
    label.set_can_target(false);
    label.set_can_focus(false);

    overlay.add_overlay(&label);
    BANNER.with(|banner| {
        *banner.borrow_mut() = Some(Banner {
            label,
            hide: None,
            last: None,
        });
    });
}

/// Say something across the window.
pub fn say(tone: Tone, text: &str) {
    BANNER.with(|cell| {
        let mut held = cell.borrow_mut();
        let Some(banner) = held.as_mut() else {
            return;
        };
        if let Some(previous) = banner.hide.take() {
            previous.remove();
        }
        for other in ["rejected", "won", "lost", "drawn", "ended"] {
            banner.label.remove_css_class(other);
        }
        banner.label.add_css_class(tone.css());
        banner.label.set_label(text);
        banner.label.set_visible(true);
        banner.last = Some((tone, text.to_owned()));

        if tone.is_transient() {
            banner.hide = Some(glib::timeout_add_local_once(
                Duration::from_millis(TRANSIENT_MS),
                || {
                    BANNER.with(|cell| {
                        if let Some(banner) = cell.borrow_mut().as_mut() {
                            banner.label.set_visible(false);
                            banner.hide = None;
                        }
                    });
                },
            ));
        }
    });
}

/// Take the message down, which every board does when it starts a new position.
pub fn clear() {
    BANNER.with(|cell| {
        let mut held = cell.borrow_mut();
        let Some(banner) = held.as_mut() else {
            return;
        };
        if let Some(previous) = banner.hide.take() {
            previous.remove();
        }
        banner.label.set_visible(false);
        banner.last = None;
    });
}

/// What was last announced, if anything is showing. For the self-test.
pub fn last() -> Option<(Tone, String)> {
    BANNER.with(|cell| {
        cell.borrow().as_ref().and_then(|banner| {
            banner
                .label
                .is_visible()
                .then(|| banner.last.clone())
                .flatten()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A refused move is a moment and clears itself; a result is a state and
    /// stays until the next position replaces it.
    #[test]
    fn only_a_rejection_clears_itself() {
        assert!(Tone::Rejected.is_transient());
        for tone in [Tone::Won, Tone::Lost, Tone::Drawn, Tone::Ended] {
            assert!(!tone.is_transient(), "{tone:?} should stay up");
        }
    }

    /// Each tone needs its own class, or they cannot be told apart and the
    /// class removal above would leave the wrong colour behind.
    #[test]
    fn every_tone_has_its_own_class() {
        let mut seen: Vec<&str> = [
            Tone::Rejected,
            Tone::Won,
            Tone::Lost,
            Tone::Drawn,
            Tone::Ended,
        ]
        .iter()
        .map(|t| t.css())
        .collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), before);
    }
}
