//! OMACHESS — a chess study application for Omarchy.

mod board;
mod charts;
mod engine_worker;
mod pieces;
mod play_view;
mod sound;
mod progress_view;
mod style;
mod trainer;

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Orientation, ScrolledWindow};
use libadwaita as adw;

use omachess_core::grade::RATING_FLOOR;
use omachess_core::{ingest, paths, store::Store};
use trainer::Trainer;

const APP_ID: &str = "dev.omachess.Omachess";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("ingest") => command_ingest(args.get(1).map(PathBuf::from)),
        Some("status") => command_status(),
        Some("progress") => command_progress(),
        Some("games") => command_games(),
        Some("--help" | "-h") => {
            print_usage();
            Ok(())
        }
        _ => run_app(),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("omachess: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    println!(
        "omachess — chess study for Omarchy

USAGE:
    omachess                 Open the trainer
    omachess ingest <FILE>   Load lichess_db_puzzle.csv.zst (puzzles rated {RATING_FLOOR}+)
    omachess status          Report what is stored
    omachess progress        Show the time-to-solve trend per rating band
    omachess games           Show how well you have been playing the engine

The puzzle export is CC0 and lives at
    {}
",
        ingest::PUZZLE_DB_URL
    );
}

fn open_store() -> anyhow::Result<Store> {
    Store::open(&paths::db_path())
}

fn command_ingest(path: Option<PathBuf>) -> anyhow::Result<()> {
    let Some(path) = path else {
        anyhow::bail!("usage: omachess ingest <lichess_db_puzzle.csv.zst>");
    };
    let mut store = open_store()?;
    println!("Reading {}…", path.display());
    let mut last = std::time::Instant::now();
    let report = ingest::ingest_default(&mut store, &path, |progress| {
        // Throttle so a fast disk does not turn the terminal into a flipbook.
        if last.elapsed() >= std::time::Duration::from_millis(500) {
            last = std::time::Instant::now();
            print!("\r  {} read, {} kept…", progress.read, progress.kept);
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
    })?;
    println!();
    println!(
        "Read {} rows: kept {}, below {RATING_FLOOR} {}, unusable {}.",
        report.read, report.kept, report.below_floor, report.malformed
    );
    println!("{} puzzles stored at {}", store.count_puzzles()?, paths::db_path().display());
    Ok(())
}

fn command_status() -> anyhow::Result<()> {
    let store = open_store()?;
    println!("database   {}", paths::db_path().display());
    println!("profile    {}", if paths::is_dev_profile() { "dev" } else { "default" });
    println!("puzzles    {}", store.count_puzzles()?);
    println!("due now    {}", store.due_count(chrono::Utc::now())?);
    println!("rating     {:.0}", store.personal_rating()?);
    match pieces::PieceSet::discover(&paths::pieces_dir()) {
        Some(set) => println!("pieces     {}", set.name()),
        None => println!("pieces     none installed (using glyphs)"),
    }
    Ok(())
}

fn command_progress() -> anyhow::Result<()> {
    use omachess_core::progress::{improvement_by_band, measured_improvement};

    let store = open_store()?;
    let seconds = |d: chrono::Duration| d.num_milliseconds() as f64 / 1000.0;

    println!(
        "mode       {}",
        if store.repeat_mode()? { "repeat (measuring)" } else { "learn (not measured)" }
    );
    println!("solved     {} distinct puzzles", store.solved_count()?);

    let Some(overall) = measured_improvement(&store)? else {
        println!(
            "\nNothing measured yet. Progress is only computed from puzzles solved more\n\
             than once, so turn on Repeat in the Train tab and re-solve some."
        );
        return Ok(());
    };

    let direction = if overall.median_speedup >= 1.0 { "faster" } else { "slower" };
    println!(
        "\n{:.0}% {direction}   {:.1}s -> {:.1}s   over {} repeated puzzles",
        (overall.median_speedup - 1.0).abs() * 100.0,
        seconds(overall.median_first),
        seconds(overall.median_latest),
        overall.puzzles,
    );
    println!(
        "{} faster, {} slower, {} unchanged   (sign test p = {:.4})",
        overall.faster, overall.slower, overall.unchanged, overall.p_value
    );

    let (rate, attempts) = store.repeat_accuracy()?;
    if attempts > 0 {
        println!("accuracy on repeats: {:.0}% of {attempts} attempts", rate * 100.0);
    }

    let bands = improvement_by_band(&store)?;
    if bands.len() > 1 {
        println!();
        for (band, result) in bands {
            println!(
                "  {band}-{}  {:.1}s -> {:.1}s  {:.0}% {}  ({} puzzles, p = {:.3})",
                band + 99,
                seconds(result.median_first),
                seconds(result.median_latest),
                (result.median_speedup - 1.0).abs() * 100.0,
                if result.median_speedup >= 1.0 { "faster" } else { "slower" },
                result.puzzles,
                result.p_value,
            );
        }
    }
    Ok(())
}

fn command_games() -> anyhow::Result<()> {
    use omachess_core::progress::{play_trend, MIN_GAMES};

    let store = open_store()?;
    let games = store.games()?;
    if games.is_empty() {
        println!("No games played yet.");
        return Ok(());
    }

    println!("{} game{} recorded\n", games.len(), if games.len() == 1 { "" } else { "s" });
    for game in games.iter().rev().take(10) {
        println!(
            "  {}  {:<5} vs {:<5}  accuracy {:>5.1}%  {:.1}% lost/move  {} blunder{}",
            game.played_at.format("%Y-%m-%d %H:%M"),
            game.result,
            game.opponent_elo,
            game.accuracy,
            game.mean_loss * 100.0,
            game.blunders,
            if game.blunders == 1 { "" } else { "s" },
        );
    }

    match play_trend(&store)? {
        Some(trend) => {
            println!(
                "\naccuracy   {:.1}% -> {:.1}%   (median of the earlier and later halves of {} games)",
                trend.earlier_accuracy, trend.recent_accuracy, trend.games
            );
            println!(
                "blunders   {:.1} -> {:.1} per 100 moves",
                trend.earlier_blunders_per_100, trend.recent_blunders_per_100
            );
            println!(
                "{}  (Mann-Whitney p = {:.3})",
                if trend.is_significant() {
                    "Unlikely to be chance."
                } else {
                    "Not yet distinguishable from chance."
                },
                trend.p_value
            );
        }
        None => println!(
            "\nNo trend yet — {MIN_GAMES} games are needed before earlier and later\n\
             halves can be compared."
        ),
    }
    Ok(())
}

fn run_app() -> anyhow::Result<()> {
    // Fail before opening a window if the database is unusable.
    open_store()?;

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| {
        // Activating an already-running instance must raise the window it has,
        // not build a second one against the same database.
        if let Some(window) = app.windows().first() {
            window.present();
            return;
        }
        if let Err(e) = build_window(app) {
            eprintln!("omachess: {e:#}");
        }
    });
    app.run_with_args::<&str>(&[]);
    Ok(())
}

fn build_window(app: &adw::Application) -> anyhow::Result<()> {
    style::install();

    let pieces = pieces::PieceSet::discover(&paths::pieces_dir()).map(Rc::new);
    match &pieces {
        Some(set) => eprintln!("omachess: piece set '{}'", set.name()),
        None => eprintln!(
            "omachess: no piece set in {} — using glyphs",
            paths::pieces_dir().display()
        ),
    }

    let store = Rc::new(RefCell::new(open_store()?));
    let engine = omachess_core::engine::find_engine();
    match &engine {
        Some(path) => eprintln!("omachess: engine {}", path.display()),
        None => eprintln!("omachess: no engine on PATH — play is unavailable"),
    }

    let sounds = Rc::new(sound::Sounds::new());
    if !sounds.is_audible() {
        eprintln!("omachess: no audio player found — running silent");
    }

    let trainer = Trainer::new(store.clone(), pieces.clone(), sounds.clone());
    let play = play_view::PlayView::new(store, pieces, sounds, engine);

    let progress = progress_view::ProgressView::new();
    progress.refresh(&trainer.progress_data());
    let progress_page = ScrolledWindow::builder().child(progress.widget()).build();

    let stack = adw::ViewStack::new();
    stack.add_titled_with_icon(
        trainer.widget(),
        Some("train"),
        "Train",
        "applications-games-symbolic",
    );
    stack.add_titled_with_icon(
        play.widget(),
        Some("play"),
        "Play",
        "media-playback-start-symbolic",
    );
    stack.add_titled_with_icon(
        &progress_page,
        Some("progress"),
        "Progress",
        "utilities-system-monitor-symbolic",
    );

    // The report only means anything once solves exist, so rebuild it on view.
    //
    // This closure also owns the two view objects. GTK keeps the widgets alive
    // by refcount, but the structs behind them are plain `Rc`s, and every
    // handler holds only a `Weak` back-reference to avoid a cycle. Without a
    // strong reference living as long as the window, both views are dropped the
    // moment this function returns and their handlers go quietly dead.
    {
        let trainer = trainer.clone();
        let play = play.clone();
        stack.connect_visible_child_name_notify(move |stack| {
            let _keep_alive = &play;
            if stack.visible_child_name().as_deref() == Some("progress") {
                progress.refresh(&trainer.progress_data());
            }
        });
    }

    let title = adw::WindowTitle::new("OMACHESS", &trainer.summary());
    let switcher = adw::ViewSwitcher::builder()
        .stack(&stack)
        .policy(adw::ViewSwitcherPolicy::Narrow)
        .build();

    let header = adw::HeaderBar::builder().title_widget(&title).build();
    header.pack_end(&switcher);

    let layout = GtkBox::builder().orientation(Orientation::Vertical).build();
    layout.append(&header);
    layout.append(&stack);
    stack.set_vexpand(true);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("OMACHESS")
        .default_width(720)
        .default_height(820)
        .content(&layout)
        .build();

    window.present();
    Ok(())
}
