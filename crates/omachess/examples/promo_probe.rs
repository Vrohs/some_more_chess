use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, Grid, Label, Orientation, Overlay,
    Popover, PositionType,
};

fn main() {
    let mode: String = std::env::args().nth(1).unwrap_or_else(|| "plain".into());
    let app = Application::builder()
        .application_id("dev.omachess.probe")
        .build();
    app.connect_activate(move |app| {
        let grid = Grid::new();
        for r in 0..8 {
            for c in 0..8 {
                grid.attach(
                    &GtkBox::builder()
                        .width_request(48)
                        .height_request(48)
                        .build(),
                    c,
                    r,
                    1,
                    1,
                );
            }
        }
        let overlay = Overlay::builder().child(&grid).build();
        let outer = GtkBox::builder().orientation(Orientation::Vertical).build();
        outer.append(&overlay);
        let focusable = Button::with_label("focus me");
        outer.append(&focusable);
        let win = ApplicationWindow::builder()
            .application(app)
            .child(&outer)
            .build();
        win.present();
        focusable.grab_focus();

        let overlay2 = overlay.clone();
        let mode = mode.clone();
        gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
            let autohide = mode != "noautohide";
            let pop = Popover::builder()
                .position(PositionType::Top)
                .autohide(autohide)
                .build();
            pop.set_child(Some(&Label::new(Some("Promote to"))));
            pop.set_parent(&overlay2);
            pop.popup();
            println!(
                "[{mode}] t=0    visible={} mapped={}",
                pop.is_visible(),
                pop.is_mapped()
            );
            let p = pop.clone();
            gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(700), move || {
                println!(
                    "[t=700] visible={} mapped={}",
                    p.is_visible(),
                    p.is_mapped()
                );
                p.unparent();
                std::process::exit(0);
            });
        });
    });
    app.run_with_args::<&str>(&[]);
}
