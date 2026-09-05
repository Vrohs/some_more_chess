//! The chess board.
//!
//! Squares are ordinary widgets carrying CSS classes rather than a cairo
//! drawing, which keeps the layout and the styling in one place. Piece art is
//! drawn from a piece set when one is installed, and from Unicode glyphs
//! otherwise.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk4::prelude::*;
use gtk4::{
    gdk, Align, Box as GtkBox, Button, EventControllerKey, Fixed, GestureDrag, GestureClick, Grid,
    Label, Orientation, Overlay, Picture,
};
use shakmaty::{Chess, Color, Position, Role, Square};

use crate::pieces::PieceSet;

type SquareHandler = Box<dyn Fn(Square)>;

/// What a press-and-release on the board meant.
///
/// A single gesture serves both interaction styles, so the decision between
/// them is worth stating once, in a form that can be checked, rather than
/// living inside an event closure where it cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interaction {
    /// Pressed and released on the same square, or pressed off the board.
    Click(Square),
    /// Pressed on one square and released on another.
    Drag { from: Square, to: Square },
    /// Nothing usable happened — released off the board, for instance.
    Nothing,
}

/// Decide what a press at one square and a release at another meant.
pub fn resolve_interaction(press: Option<Square>, release: Option<Square>) -> Interaction {
    match (press, release) {
        (Some(from), Some(to)) if from != to => Interaction::Drag { from, to },
        (_, Some(to)) => Interaction::Click(to),
        _ => Interaction::Nothing,
    }
}
type DragHandler = Box<dyn Fn(Square, Square)>;

/// However a piece is drawn, it is held directly rather than found by walking
/// the widget tree on every redraw.
enum PieceWidget {
    Art(Picture),
    Glyph(Label),
}

struct SquareWidgets {
    cell: GtkBox,
    piece: PieceWidget,
    /// Rank digit, shown only while this square sits in the leftmost column.
    rank: Label,
    /// File letter, shown only while this square sits in the bottom row.
    file: Label,
}

pub struct BoardView {
    /// The grid, plus the piece that follows the cursor while dragging.
    root: Overlay,
    /// The dragged piece lives in a `Fixed` so it is positioned by coordinates
    /// rather than margins, and the overlay is told not to measure it. Between
    /// them that guarantees dragging cannot change the board's layout — if it
    /// did, the board would resize under the pointer mid-drag and the square
    /// released on would not be the square under the cursor.
    ghost_layer: Fixed,
    ghost: Picture,
    /// The promotion picker, drawn on the board rather than in a popup.
    ///
    /// A `GtkPopover` was tried first and could not be trusted here: an
    /// autohide popover dismisses itself the moment it fails to take a grab,
    /// which it does when it is raised out of the drag gesture that asked for
    /// it. The pawn then stayed on its old square with nothing on screen to
    /// say why. Ordinary widgets inside the board's own overlay take no grab,
    /// so they cannot disappear between being shown and being clicked.
    promo_layer: Fixed,
    /// Covers the whole board while the picker is up, so a stray click
    /// cancels the promotion instead of starting another move underneath it.
    promo_shade: GtkBox,
    promo_column: GtkBox,
    /// The offered pieces in the order they are drawn, so a test can click
    /// one by name rather than by guessing which child it is.
    promo_roles: RefCell<Vec<Role>>,
    grid: Grid,
    squares: Vec<SquareWidgets>,
    handler: RefCell<Option<SquareHandler>>,
    drag_handler: RefCell<Option<DragHandler>>,
    selected: RefCell<Option<Square>>,
    /// The move just played, highlighted so the reply is easy to see.
    last_move: RefCell<Option<(Square, Square)>>,
    /// The king square to mark when the side to move is in check.
    check: RefCell<Option<Square>>,
    /// The mated king, if the game ended that way.
    mate: RefCell<Option<Square>>,
    /// Which side is at the bottom of the board.
    orientation: RefCell<Color>,
    /// Square the pointer went down on, used to tell a drag from a click.
    press_origin: Cell<Option<Square>>,
    /// The keyboard cursor, so the board can be played without a mouse.
    cursor: Cell<Option<Square>>,
    /// Where the press began, in grid coordinates.
    press_point: Cell<(f64, f64)>,
    /// Edge length of the carried piece, in pixels.
    ghost_size: Cell<i32>,
    /// When absent the board falls back to Unicode glyphs.
    pieces: Option<Rc<PieceSet>>,
}

impl BoardView {
    pub fn new(pieces: Option<Rc<PieceSet>>) -> Rc<Self> {
        let grid = Grid::builder()
            .row_homogeneous(true)
            .column_homogeneous(true)
            .hexpand(true)
            .vexpand(true)
            // Focusable so the board can be played from the keyboard alone.
            .focusable(true)
            .build();
        grid.add_css_class("omachess-board");

        let mut squares = Vec::with_capacity(64);
        for index in 0..64u32 {
            let square = Square::new(index);
            let light = square_is_light(square);

            // A plain box rather than a button: a button installs its own
            // click gesture, which competes with the grid-level one and makes
            // press-and-drag impossible to detect reliably.
            let cell = GtkBox::builder().orientation(Orientation::Vertical).build();
            cell.add_css_class("omachess-square");
            cell.add_css_class(if light { "light" } else { "dark" });

            let piece = match pieces {
                Some(_) => {
                    let picture = Picture::builder()
                        .halign(Align::Fill)
                        .valign(Align::Fill)
                        .can_shrink(true)
                        .build();
                    picture.add_css_class("omachess-piece");
                    PieceWidget::Art(picture)
                }
                None => {
                    let label = Label::builder()
                        .halign(Align::Center)
                        .valign(Align::Center)
                        .build();
                    label.add_css_class("omachess-piece");
                    PieceWidget::Glyph(label)
                }
            };

            let rank = coordinate_label(
                &square.rank().char().to_string(),
                Align::Start,
                Align::Start,
                light,
            );
            let file = coordinate_label(
                &square.file().char().to_string(),
                Align::End,
                Align::End,
                light,
            );

            let overlay = Overlay::new();
            match &piece {
                PieceWidget::Art(picture) => overlay.set_child(Some(picture)),
                PieceWidget::Glyph(label) => overlay.set_child(Some(label)),
            }
            overlay.add_overlay(&rank);
            overlay.add_overlay(&file);
            overlay.set_hexpand(true);
            overlay.set_vexpand(true);
            cell.append(&overlay);

            let (column, row) = grid_position(square, Color::White);
            grid.attach(&cell, column, row, 1, 1);

            squares.push(SquareWidgets {
                cell,
                piece,
                rank,
                file,
            });
        }

        // The dragged piece is drawn above the board and follows the pointer,
        // which is what makes dragging feel like moving a piece rather than
        // issuing a command.
        let ghost = Picture::builder()
            .can_shrink(true)
            .visible(false)
            .can_target(false)
            .build();
        ghost.add_css_class("omachess-piece");
        ghost.add_css_class("omachess-ghost");

        let ghost_layer = Fixed::builder().can_target(false).build();
        ghost_layer.put(&ghost, 0.0, 0.0);

        let promo_shade = GtkBox::builder()
            .css_classes(vec!["omachess-promo-shade".to_owned()])
            .build();
        let promo_column = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .css_classes(vec!["omachess-promo".to_owned()])
            .build();
        let promo_layer = Fixed::builder().visible(false).build();
        promo_layer.put(&promo_shade, 0.0, 0.0);
        promo_layer.put(&promo_column, 0.0, 0.0);

        let root = Overlay::builder().child(&grid).build();
        root.add_overlay(&ghost_layer);
        root.set_measure_overlay(&ghost_layer, false);
        root.add_overlay(&promo_layer);
        root.set_measure_overlay(&promo_layer, false);

        let view = Rc::new(Self {
            root,
            ghost_layer,
            ghost,
            promo_layer,
            promo_shade,
            promo_column,
            promo_roles: RefCell::new(Vec::new()),
            grid,
            squares,
            handler: RefCell::new(None),
            drag_handler: RefCell::new(None),
            selected: RefCell::new(None),
            last_move: RefCell::new(None),
            check: RefCell::new(None),
            mate: RefCell::new(None),
            orientation: RefCell::new(Color::White),
            press_origin: Cell::new(None),
            cursor: Cell::new(None),
            press_point: Cell::new((0.0, 0.0)),
            ghost_size: Cell::new(0),
            pieces,
        });
        view.apply_coordinates(Color::White);

        // A single drag gesture covers both styles: releasing on the square you
        // pressed is a click, releasing elsewhere is a drag. Two gestures would
        // fight over the same event sequence.
        let drag = GestureDrag::new();

        let weak: Weak<Self> = Rc::downgrade(&view);
        drag.connect_drag_begin(move |_, x, y| {
            if let Some(view) = weak.upgrade() {
                view.begin_drag(x, y);
            }
        });

        let weak: Weak<Self> = Rc::downgrade(&view);
        drag.connect_drag_update(move |_, dx, dy| {
            if let Some(view) = weak.upgrade() {
                view.update_drag(dx, dy);
            }
        });

        let weak: Weak<Self> = Rc::downgrade(&view);
        drag.connect_drag_end(move |_, dx, dy| {
            let Some(view) = weak.upgrade() else {
                return;
            };
            let from = view.press_origin.take();
            view.end_drag();

            let (sx, sy) = view.press_point.get();
            let to = view.square_at_point(sx + dx, sy + dy);
            match resolve_interaction(from, to) {
                Interaction::Drag { from, to } => view.on_drag(from, to),
                Interaction::Click(square) => view.on_click(square),
                Interaction::Nothing => {}
            }
        });
        view.grid.add_controller(drag);

        // Arrow keys walk a cursor, Enter or Space acts on it — the same two
        // steps a click makes, for anyone not using a mouse.
        let keys = EventControllerKey::new();
        let weak: Weak<Self> = Rc::downgrade(&view);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(view) = weak.upgrade() else {
                return gtk4::glib::Propagation::Proceed;
            };
            let step = match key {
                gdk::Key::Left => (-1, 0),
                gdk::Key::Right => (1, 0),
                gdk::Key::Up => (0, -1),
                gdk::Key::Down => (0, 1),
                gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space => {
                    if let Some(square) = view.cursor.get() {
                        view.on_click(square);
                    }
                    return gtk4::glib::Propagation::Stop;
                }
                gdk::Key::Escape => {
                    view.select(None);
                    return gtk4::glib::Propagation::Stop;
                }
                _ => return gtk4::glib::Propagation::Proceed,
            };
            view.move_cursor(step.0, step.1);
            gtk4::glib::Propagation::Stop
        });
        view.grid.add_controller(keys);

        // Anywhere on the board that is not one of the offered pieces cancels
        // the promotion, leaving the pawn where it was. A move the player did
        // not choose is worse than no move at all.
        let cancel = GestureClick::new();
        let weak: Weak<Self> = Rc::downgrade(&view);
        cancel.connect_pressed(move |_, _, _, _| {
            if let Some(view) = weak.upgrade() {
                view.dismiss_promotion();
            }
        });
        view.promo_shade.add_controller(cancel);

        view
    }

    /// Offer the pieces a pawn can become, as a column on the board itself.
    ///
    /// The column starts on the promotion square and runs into the board,
    /// which is where every chess site puts it, so the piece being chosen sits
    /// under the pointer that just dragged the pawn there.
    pub fn ask_promotion(
        self: &Rc<Self>,
        to: Square,
        white: bool,
        choices: &[Role],
        chosen: impl Fn(Role) + 'static,
    ) {
        self.dismiss_promotion();
        if choices.is_empty() {
            return;
        }

        let allocation = self.squares[to as usize].cell.allocation();
        let size = allocation.width().min(allocation.height());
        if size <= 0 {
            // Nothing has been allocated yet, so there is no board to draw on
            // and no coordinates to draw at.
            return;
        }

        // Rank 8 and rank 1 are always the top and bottom rows on screen, so
        // the only question is which way the column has room to run.
        let downward = grid_position(to, *self.orientation.borrow()).1 == 0;
        let top = if downward {
            allocation.y()
        } else {
            allocation.y() - (choices.len() as i32 - 1) * size
        };

        self.promo_shade
            .set_size_request(self.grid.width(), self.grid.height());

        // Whichever way the column runs, the first choice — the queen — is the
        // one touching the promotion square, so the common case is the nearest.
        let ordered: Vec<Role> = if downward {
            choices.to_vec()
        } else {
            choices.iter().rev().copied().collect()
        };
        let colour = if white { Color::White } else { Color::Black };
        let chosen = Rc::new(chosen);
        for role in ordered {
            let button = Button::builder()
                .width_request(size)
                .height_request(size)
                .css_classes(vec!["omachess-promo-choice".to_owned()])
                .build();
            match self.pieces.as_ref().and_then(|set| set.texture(colour, role)) {
                Some(texture) => {
                    let art = Picture::builder()
                        .paintable(texture)
                        .can_shrink(true)
                        .halign(Align::Fill)
                        .valign(Align::Fill)
                        .build();
                    art.add_css_class("omachess-piece");
                    button.set_child(Some(&art));
                }
                None => {
                    let label = Label::new(Some(&glyph(role).to_string()));
                    label.add_css_class("omachess-piece");
                    label.add_css_class(if white { "white-piece" } else { "black-piece" });
                    button.set_child(Some(&label));
                }
            }
            let chosen = chosen.clone();
            let weak: Weak<Self> = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                // Taken down first: the handler redraws the board, and the
                // picker must not still be sitting over the new position.
                if let Some(view) = weak.upgrade() {
                    view.dismiss_promotion();
                }
                chosen(role);
            });
            self.promo_column.append(&button);
            self.promo_roles.borrow_mut().push(role);
        }

        self.promo_layer.move_(&self.promo_shade, 0.0, 0.0);
        self.promo_layer
            .move_(&self.promo_column, f64::from(allocation.x()), f64::from(top));
        self.promo_layer.set_visible(true);
    }

    /// Take the picker down, dropping the choice handler with it.
    pub fn dismiss_promotion(&self) {
        self.promo_layer.set_visible(false);
        self.promo_roles.borrow_mut().clear();
        while let Some(child) = self.promo_column.first_child() {
            self.promo_column.remove(&child);
        }
    }

    /// Whether the picker is on screen. The bug worth catching is a picker
    /// that was built and never shown, which no rules test can see.
    pub(crate) fn promotion_showing(&self) -> bool {
        self.promo_layer.is_visible()
    }

    pub(crate) fn promotion_choice_count(&self) -> usize {
        self.promo_roles.borrow().len()
    }

    /// Click an offered piece, exactly as the pointer would.
    pub(crate) fn click_promotion_choice(&self, role: Role) -> Result<(), String> {
        let index = self
            .promo_roles
            .borrow()
            .iter()
            .position(|offered| *offered == role)
            .ok_or_else(|| format!("{role:?} was not offered"))?;
        let mut child = self
            .promo_column
            .first_child()
            .ok_or("the picker had no choices in it")?;
        for _ in 0..index {
            child = child.next_sibling().ok_or("the picker was short a choice")?;
        }
        child
            .downcast_ref::<Button>()
            .ok_or("a promotion choice was not a button")?
            .emit_clicked();
        Ok(())
    }

    pub fn widget(&self) -> &Overlay {
        &self.root
    }

    /// Put `side` at the bottom of the board.
    ///
    /// A solver reads far more slowly from the wrong side, so a puzzle where
    /// Black is to move must be shown from Black's point of view — otherwise
    /// the recorded solve time measures disorientation rather than chess.
    pub fn set_orientation(&self, side: Color) {
        if self.orientation.replace(side) == side {
            return;
        }
        for index in 0..64u32 {
            let square = Square::new(index);
            let cell = &self.squares[index as usize].cell;
            let (column, row) = grid_position(square, side);
            self.grid.remove(cell);
            self.grid.attach(cell, column, row, 1, 1);
        }
        self.apply_coordinates(side);
    }

    /// Show each coordinate only on the edge it labels, for this orientation.
    fn apply_coordinates(&self, side: Color) {
        for index in 0..64u32 {
            let square = Square::new(index);
            let (column, row) = grid_position(square, side);
            let widgets = &self.squares[index as usize];
            widgets.rank.set_visible(column == 0);
            widgets.file.set_visible(row == 7);
        }
    }

    /// Called with each square the user activates.
    pub fn connect_move(self: &Rc<Self>, handler: impl Fn(Square) + 'static) {
        *self.handler.borrow_mut() = Some(Box::new(handler));
    }

    /// Click a square, exactly as a press and release on it would.
    ///
    /// This exists so a test can enter through the board rather than through
    /// the view's handler: the bug worth catching is a handler that was never
    /// connected, and calling the handler directly cannot see that.
    pub(crate) fn click(&self, square: Square) {
        self.on_click(square);
    }

    fn on_click(&self, square: Square) {
        if let Some(handler) = self.handler.borrow().as_ref() {
            handler(square);
        }
    }

    /// Which square lies under a point in grid coordinates.
    ///
    /// Hit-testing by picking the widget is exact, where dividing the grid
    /// width by eight would drift with padding and border widths.
    fn square_at_point(&self, x: f64, y: f64) -> Option<Square> {
        let mut widget = self.grid.pick(x, y, gtk4::PickFlags::DEFAULT)?;
        loop {
            if let Some(index) = self
                .squares
                .iter()
                .position(|s| s.cell.upcast_ref::<gtk4::Widget>() == &widget)
            {
                return Some(Square::new(index as u32));
            }
            widget = widget.parent()?;
        }
    }

    /// Pick a piece up: mark the square, lift its image onto the ghost, and
    /// leave the square looking empty until the drag finishes.
    fn begin_drag(&self, x: f64, y: f64) {
        self.press_point.set((x, y));
        let Some(square) = self.square_at_point(x, y) else {
            self.press_origin.set(None);
            return;
        };
        self.press_origin.set(Some(square));
        self.squares[square as usize].cell.add_css_class("pressed");

        if let PieceWidget::Art(picture) = &self.squares[square as usize].piece {
            let Some(paintable) = picture.paintable() else {
                return;
            };
            // Exactly one square, so the piece is the same size in the hand as
            // it was on the board.
            let size = picture.width().min(picture.height());
            if size <= 0 {
                return;
            }
            self.ghost.set_paintable(Some(&paintable));
            self.ghost.set_size_request(size, size);
            self.ghost_size.set(size);
            picture.set_opacity(0.25);
            self.place_ghost(x, y);
            self.ghost.set_visible(true);
        }
    }

    fn update_drag(&self, dx: f64, dy: f64) {
        if !self.ghost.is_visible() {
            return;
        }
        let (sx, sy) = self.press_point.get();
        self.place_ghost(sx + dx, sy + dy);
    }

    /// Put the dragged piece down and restore the board's own rendering.
    fn end_drag(&self) {
        self.ghost.set_visible(false);
        for square in &self.squares {
            square.cell.remove_css_class("pressed");
            if let PieceWidget::Art(picture) = &square.piece {
                picture.set_opacity(1.0);
            }
        }
    }

    /// Centre the carried piece on the pointer.
    fn place_ghost(&self, x: f64, y: f64) {
        let size = f64::from(self.ghost_size.get().max(1));
        self.ghost_layer
            .move_(&self.ghost, x - size / 2.0, y - size / 2.0);
    }

    /// A piece dragged from one square to another.
    pub fn connect_drag(self: &Rc<Self>, handler: impl Fn(Square, Square) + 'static) {
        *self.drag_handler.borrow_mut() = Some(Box::new(handler));
    }

    fn on_drag(&self, from: Square, to: Square) {
        if let Some(handler) = self.drag_handler.borrow().as_ref() {
            handler(from, to);
        }
    }

    /// Draw a position.
    pub fn set_position(&self, position: &Chess) {
        let board = position.board();
        for index in 0..64u32 {
            let square = Square::new(index);
            let piece = board.piece_at(square);

            match (&self.squares[index as usize].piece, &self.pieces) {
                (PieceWidget::Art(picture), Some(set)) => match piece {
                    Some(piece) => picture.set_paintable(set.texture(piece.color, piece.role)),
                    None => picture.set_paintable(gtk4::gdk::Paintable::NONE),
                },
                (PieceWidget::Glyph(label), _) => match piece {
                    Some(piece) => {
                        label.set_text(&glyph(piece.role).to_string());
                        let (add, remove) = match piece.color {
                            Color::White => ("white-piece", "black-piece"),
                            Color::Black => ("black-piece", "white-piece"),
                        };
                        label.remove_css_class(remove);
                        label.add_css_class(add);
                    }
                    None => {
                        label.set_text("");
                        label.remove_css_class("white-piece");
                        label.remove_css_class("black-piece");
                    }
                },
                (PieceWidget::Art(_), None) => {}
            }
        }
    }

    /// Move the keyboard cursor, starting from the middle if it has none yet.
    fn move_cursor(&self, dx: i32, dy: i32) {
        let orientation = *self.orientation.borrow();
        let next = match self.cursor.get() {
            Some(square) => step_cursor(square, dx, dy, orientation).unwrap_or(square),
            // The first key press lands somewhere central rather than a corner.
            None => square_at(4, 4, orientation).unwrap_or(Square::E4),
        };
        self.set_cursor(Some(next));
    }

    fn set_cursor(&self, square: Option<Square>) {
        if let Some(previous) = self.cursor.replace(square) {
            self.squares[previous as usize]
                .cell
                .remove_css_class("cursor");
        }
        if let Some(square) = square {
            self.squares[square as usize].cell.add_css_class("cursor");
        }
    }

    /// Empty every square. Used to withhold a puzzle until the solver is ready,
    /// so the clock and the position start together.
    pub fn clear(&self) {
        for index in 0..64u32 {
            match &self.squares[index as usize].piece {
                PieceWidget::Art(picture) => picture.set_paintable(gtk4::gdk::Paintable::NONE),
                PieceWidget::Glyph(label) => {
                    label.set_text("");
                    label.remove_css_class("white-piece");
                    label.remove_css_class("black-piece");
                }
            }
        }
    }

    pub fn selected(&self) -> Option<Square> {
        *self.selected.borrow()
    }

    /// Mark a square as the origin of a move in progress.
    pub fn select(&self, square: Option<Square>) {
        if let Some(previous) = self.selected.replace(square) {
            self.squares[previous as usize]
                .cell
                .remove_css_class("selected");
        }
        if let Some(square) = square {
            self.squares[square as usize].cell.add_css_class("selected");
        }
    }

    /// Highlight the move just played.
    pub fn set_last_move(&self, mv: Option<(Square, Square)>) {
        if let Some((from, to)) = self.last_move.replace(mv) {
            self.squares[from as usize]
                .cell
                .remove_css_class("last-move");
            self.squares[to as usize].cell.remove_css_class("last-move");
        }
        if let Some((from, to)) = mv {
            self.squares[from as usize].cell.add_css_class("last-move");
            self.squares[to as usize].cell.add_css_class("last-move");
        }
    }

    /// Mark a king as mated — deliberately louder than check, because the game
    /// being over is the single most important thing the board can say.
    pub fn set_mate(&self, square: Option<Square>) {
        if let Some(previous) = self.mate.replace(square) {
            self.squares[previous as usize]
                .cell
                .remove_css_class("mated");
        }
        if let Some(square) = square {
            self.squares[square as usize].cell.add_css_class("mated");
        }
    }

    /// Mark a king in check.
    pub fn set_check(&self, square: Option<Square>) {
        if let Some(previous) = self.check.replace(square) {
            self.squares[previous as usize]
                .cell
                .remove_css_class("in-check");
        }
        if let Some(square) = square {
            self.squares[square as usize].cell.add_css_class("in-check");
        }
    }
}

fn coordinate_label(text: &str, halign: Align, valign: Align, on_light: bool) -> Label {
    let label = Label::builder()
        .label(text)
        .halign(halign)
        .valign(valign)
        .margin_start(3)
        .margin_end(3)
        .margin_top(1)
        .margin_bottom(1)
        .build();
    label.add_css_class("omachess-coord");
    // Coordinates sit on the square they label, so they take their contrast
    // from that square rather than from the board as a whole.
    label.add_css_class(if on_light { "on-light" } else { "on-dark" });
    label
}

/// The square shown at a grid cell, or `None` off the board. The inverse of
/// [`grid_position`], used to walk the keyboard cursor in the direction the
/// player sees rather than the direction the ranks run.
pub fn square_at(column: i32, row: i32, orientation: Color) -> Option<Square> {
    if !(0..8).contains(&column) || !(0..8).contains(&row) {
        return None;
    }
    let (file, rank) = match orientation {
        Color::White => (column, 7 - row),
        Color::Black => (7 - column, row),
    };
    Some(Square::from_coords(
        shakmaty::File::new(file as u32),
        shakmaty::Rank::new(rank as u32),
    ))
}

/// Step the keyboard cursor one square in a screen direction.
///
/// `dx` and `dy` are in screen terms — right and down — so the same key does
/// the same visible thing whichever way the board is facing.
pub fn step_cursor(from: Square, dx: i32, dy: i32, orientation: Color) -> Option<Square> {
    let (column, row) = grid_position(from, orientation);
    square_at(column + dx, row + dy, orientation)
}

/// Where a square sits in the grid for a given orientation.
fn grid_position(square: Square, orientation: Color) -> (i32, i32) {
    let file = i32::from(square.file().char() as u8 - b'a');
    let rank = i32::from(square.rank().char() as u8 - b'1');
    match orientation {
        // White at the bottom: rank 8 on top, file a on the left.
        Color::White => (file, 7 - rank),
        Color::Black => (7 - file, rank),
    }
}

fn square_is_light(square: Square) -> bool {
    let file = square.file().char() as u8 - b'a';
    let rank = square.rank().char() as u8 - b'1';
    (file + rank) % 2 == 1
}

/// Solid Unicode chess glyphs for both sides, used when no piece set is
/// installed. Colouring one glyph set per side renders more evenly than mixing
/// the outline and solid ranges, which many fonts draw at different weights.
fn glyph(role: Role) -> char {
    match role {
        Role::King => '\u{265A}',
        Role::Queen => '\u{265B}',
        Role::Rook => '\u{265C}',
        Role::Bishop => '\u{265D}',
        Role::Knight => '\u{265E}',
        Role::Pawn => '\u{265F}',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_moves_the_way_the_board_is_facing() {
        // With White at the bottom, "up" goes toward rank 8.
        assert_eq!(
            step_cursor(Square::E4, 0, -1, Color::White),
            Some(Square::E5)
        );
        // Flipped, the same key still moves up the screen, which is rank 3.
        assert_eq!(
            step_cursor(Square::E4, 0, -1, Color::Black),
            Some(Square::E3)
        );
    }

    #[test]
    fn the_cursor_moves_right_on_screen_in_both_orientations() {
        assert_eq!(
            step_cursor(Square::D4, 1, 0, Color::White),
            Some(Square::E4)
        );
        assert_eq!(
            step_cursor(Square::D4, 1, 0, Color::Black),
            Some(Square::C4)
        );
    }

    #[test]
    fn the_cursor_stops_at_the_edge_rather_than_wrapping() {
        assert_eq!(step_cursor(Square::A1, -1, 0, Color::White), None);
        assert_eq!(step_cursor(Square::H8, 0, -1, Color::White), None);
        assert_eq!(step_cursor(Square::A1, 1, 0, Color::Black), None);
    }

    #[test]
    fn square_at_is_the_exact_inverse_of_grid_position() {
        for orientation in [Color::White, Color::Black] {
            for index in 0..64u32 {
                let square = Square::new(index);
                let (column, row) = grid_position(square, orientation);
                assert_eq!(square_at(column, row, orientation), Some(square));
            }
        }
    }

    #[test]
    fn releasing_on_a_different_square_is_a_drag() {
        assert_eq!(
            resolve_interaction(Some(Square::E1), Some(Square::G1)),
            Interaction::Drag {
                from: Square::E1,
                to: Square::G1
            },
            "dragging the king two squares is how castling is played"
        );
    }

    #[test]
    fn releasing_where_you_pressed_is_a_click() {
        assert_eq!(
            resolve_interaction(Some(Square::E2), Some(Square::E2)),
            Interaction::Click(Square::E2)
        );
    }

    #[test]
    fn a_press_that_began_off_the_board_still_selects_where_it_ended() {
        assert_eq!(
            resolve_interaction(None, Some(Square::D4)),
            Interaction::Click(Square::D4)
        );
    }

    #[test]
    fn releasing_off_the_board_does_nothing() {
        assert_eq!(
            resolve_interaction(Some(Square::E2), None),
            Interaction::Nothing
        );
        assert_eq!(resolve_interaction(None, None), Interaction::Nothing);
    }

    #[test]
    fn a1_is_dark_and_h1_is_light() {
        // The near-right corner is light on every real board.
        assert!(!square_is_light(Square::A1));
        assert!(square_is_light(Square::H1));
        assert!(square_is_light(Square::A8));
        assert!(!square_is_light(Square::H8));
    }

    #[test]
    fn white_orientation_puts_a1_bottom_left() {
        assert_eq!(grid_position(Square::A1, Color::White), (0, 7));
        assert_eq!(grid_position(Square::H8, Color::White), (7, 0));
    }

    #[test]
    fn flipping_mirrors_the_board_in_both_axes() {
        assert_eq!(grid_position(Square::A1, Color::Black), (7, 0));
        assert_eq!(grid_position(Square::H8, Color::Black), (0, 7));
    }

    /// Flipping moves widgets but must never disturb the checker pattern: the
    /// square colour classes are fixed at construction, so a bug in
    /// `grid_position` would show up as two same-coloured cells side by side.
    #[test]
    fn the_checker_pattern_survives_flipping() {
        for orientation in [Color::White, Color::Black] {
            let mut cells = std::collections::HashMap::new();
            for index in 0..64u32 {
                let square = Square::new(index);
                cells.insert(grid_position(square, orientation), square_is_light(square));
            }
            assert_eq!(cells.len(), 64, "{orientation:?}: squares collided");

            for row in 0..8 {
                for col in 0..7 {
                    assert_ne!(
                        cells[&(col, row)],
                        cells[&(col + 1, row)],
                        "{orientation:?}: ({col},{row}) and ({},{row}) share a colour",
                        col + 1
                    );
                }
            }
            assert!(
                cells[&(7, 7)],
                "{orientation:?}: near-right corner must be light"
            );
        }
    }

    #[test]
    fn coordinates_label_one_full_edge_each() {
        for orientation in [Color::White, Color::Black] {
            let mut ranks = Vec::new();
            let mut files = Vec::new();
            for index in 0..64u32 {
                let square = Square::new(index);
                let (column, row) = grid_position(square, orientation);
                if column == 0 {
                    ranks.push(square.rank().char());
                }
                if row == 7 {
                    files.push(square.file().char());
                }
            }
            ranks.sort_unstable();
            files.sort_unstable();
            assert_eq!(ranks, vec!['1', '2', '3', '4', '5', '6', '7', '8']);
            assert_eq!(files, vec!['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h']);
        }
    }
}
