//! Albion Meter: one native process with a Slint UI (see rust/README.md).
//!
//! The renderer is the software one unless `--renderer femtovg`, `ALBION_METER_RENDERER=femtovg` or `"renderer":
//! "femtovg"` in settings.json asks for OpenGL; if it fails to start, the other one is tried.
//!
//! `--fake` runs the whole UI on an animated fake fight until the engine is wired in (`--fake-idle`: waiting for Play). Other flags: `--software` (no
//! OpenGL), `--fake-update` (an update downloads and becomes ready), `--view meter|capture|game|journal`, `--journal`
//! (shows the dungeon journal tab), `--lang en|pt`, `--screenshots <dir>` (saves a PNG of every screen, then quits).

// A window app on Windows, not a console one: no black console opens beside the meter. Debug builds keep the console
// for their logs; a release build still writes to a console or pipe it is started from (the CI smoke test reads those).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod controller;
mod desktop;
mod fake;
// The contract with the engine, store and AlbionLab: the integration phase uses what the fake feed does not.
#[allow(dead_code)]
mod feed;
mod fixture;
mod format;
mod headless;
mod instance;
mod i18n;
mod journal;
mod mailbox;
mod merge;
mod ranking;
mod ranking_image;
mod screenshots;
mod updater;
#[cfg(test)]
mod ui_tests;

use std::rc::Rc;
use std::sync::mpsc;

use slint::ComponentHandle;

use crate::controller::Controller;
use crate::feed::MeterFeed;
use crate::i18n::Language;

slint::include_modules!();

#[derive(Default)]
struct Options {
    fake: bool,
    idle: bool,
    software: bool,
    renderer: Option<String>,
    update: bool,
    journal: bool,
    view: Option<String>,
    language: Option<Language>,
    screenshots: Option<std::path::PathBuf>,
    screen: Option<String>,
    screenshot: Option<std::path::PathBuf>,
    fixture: Option<std::path::PathBuf>,
    replay: Option<std::path::PathBuf>,
}

fn options() -> Options {
    let mut options = Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fake" => options.fake = true,
            "--fake-idle" => {
                options.fake = true;
                options.idle = true;
            }
            "--software" => options.software = true,
            "--renderer" => options.renderer = args.next(),
            "--fake-update" => options.update = true,
            "--journal" => options.journal = true,
            "--view" => options.view = args.next(),
            "--lang" => options.language = args.next().map(|value| Language::from_locale(&value)),
            "--screenshots" => options.screenshots = args.next().map(Into::into),
            "--screen" => options.screen = args.next(),
            "--screenshot" => options.screenshot = args.next().map(Into::into),
            "--fixture" => options.fixture = args.next().map(Into::into),
            "--replay" => options.replay = args.next().map(Into::into),
            // The C# server's settings (`--Storage:Directory=…`, `--Capture:AutoStart=true`, …), read by meter-runtime.
            _ if ["--Storage:", "--Capture:", "--AlbionLab:"].iter().any(|prefix| arg.starts_with(prefix)) => {
                if !arg.contains('=') {
                    args.next();
                }
            }
            _ => eprintln!("unknown option {arg}"),
        }
    }
    options
}

/// The renderer of the winit backend (`desktop::renderer`). A renderer that cannot start makes the meter start again
/// with the other one; the backend is chosen once per process, so that is a new process.
fn select_backend(renderer: &str) -> Result<(), slint::PlatformError> {
    if std::env::var_os("SLINT_BACKEND").is_some() {
        return Ok(());
    }
    slint::BackendSelector::new().backend_name("winit".into()).renderer_name(renderer.into()).select()
}

fn fall_back(renderer: &str, error: &slint::PlatformError) -> ! {
    let other = if renderer == "software" { "femtovg" } else { "software" };
    if std::env::var_os("ALBION_METER_FALLBACK").is_none() {
        eprintln!("the {renderer} renderer did not start ({error}); starting again with {other}");
        let status = std::process::Command::new(std::env::current_exe().unwrap_or_else(|_| "meter-app".into()))
            .args(std::env::args_os().skip(1))
            .env("ALBION_METER_RENDERER", other)
            .env("ALBION_METER_FALLBACK", "1")
            .status();
        std::process::exit(status.ok().and_then(|status| status.code()).unwrap_or(1));
    }
    eprintln!("no renderer could start: {error}");
    std::process::exit(1);
}

fn main() -> Result<(), slint::PlatformError> {
    // Velopack's install/update hooks run before anything else (they may end the process).
    updater::hooks();
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--render-ranking") {
        let (input, output) = (args.next().unwrap_or_default(), args.next().unwrap_or_default());
        if let Err(error) = ranking_image::child(input.as_ref(), output.as_ref()) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let options = options();
    let fixture_path = options.fixture.clone().unwrap_or_else(fixture::default_path);
    if let (Some(screen), Some(output)) = (&options.screen, &options.screenshot) {
        if let Err(error) = headless::run(&fixture_path, screen, output) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let real = !options.fake && options.screen.is_none() && options.screenshots.is_none();
    let config = real.then(|| meter_runtime::Config::from_environment(env!("CARGO_PKG_VERSION"), &std::env::args().collect::<Vec<_>>()));
    if let (Some(config), Some(output)) = (&config, &options.screenshot) {
        let size = (1280, 820);
        if let Err(error) = headless::run_runtime(config.clone(), item_ids(config), options.replay.clone(), options.view.as_deref().unwrap_or("meter"), options.language.unwrap_or_else(desktop::language), output, size) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let screen = match options.screen.as_deref().map(|name| fixture::screen(&fixture_path, name)) {
        Some(Ok(screen)) => Some(screen),
        Some(Err(error)) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
        None => None,
    };
    // Startup of the installed meter, as desktop/main.js did before it opened its window: elevation on Windows, the
    // capture permission on Linux, then one meter per storage folder.
    let mut _instance = None;
    if let Some(config) = &config {
        desktop::elevate_on_windows();
        desktop::capture_permission_at_start();
        match instance::claim(&config.storage, || {
            let _ = slint::invoke_from_event_loop(|| controller::with_controller(|controller| controller.focus_window()));
        }) {
            instance::Instance::First(guard) => _instance = Some(guard),
            instance::Instance::Second => {
                eprintln!("Albion Meter is already running; its window was asked to come to the front.");
                return Ok(());
            }
        }
    }
    if options.software {
        // SAFETY: still single-threaded, before anything reads the environment.
        unsafe { std::env::set_var("ALBION_METER_RENDERER", "software") };
    }
    if let Some(renderer) = &options.renderer {
        // SAFETY: as above.
        unsafe { std::env::set_var("ALBION_METER_RENDERER", renderer) };
    }
    let renderer = desktop::renderer();
    let app = match select_backend(&renderer).and_then(|()| AppWindow::new()) {
        Ok(app) => app,
        Err(error) => fall_back(&renderer, &error),
    };
    let (sans, mono) = desktop::fonts();
    app.global::<Theme>().set_sans(sans.into());
    app.global::<Theme>().set_mono(mono.into());
    if let Some(placement) = desktop::placement() {
        eprintln!("window: restoring {}x{}{}", placement.width, placement.height, if placement.maximized { ", maximized" } else { "" });
        app.window().set_size(slint::LogicalSize::new(placement.width, placement.height));
        if placement.maximized {
            app.window().set_maximized(true);
        }
    }
    let shell = app.global::<Shell>();
    shell.set_journal_enabled(options.journal || options.view.as_deref() == Some("journal"));
    if let Some(view) = &options.view {
        shell.set_view(view.as_str().into());
    }

    let (commands, received) = mpsc::channel();
    let language = options.language.or(screen.as_ref().map(|screen| Language::from_locale(&screen.ui.language))).unwrap_or_else(desktop::language);
    let controller = Controller::new(&app, commands, language);
    controller.borrow_mut().persist = options.screenshots.is_none() && options.screen.is_none();

    let mut feed_thread = None;
    if let Some(screen) = screen {
        // One fixture screen in a real window: its events, then the UI state the React side clicks into.
        controller.borrow_mut().journal.item_ids = screen.item_ids.clone();
        for event in screen.events {
            controller.borrow_mut().handle(event);
        }
        headless::apply(&app, &screen.ui);
        let (weak, ui) = (app.as_weak(), screen.ui);
        slint::Timer::single_shot(std::time::Duration::from_millis(300), move || {
            if let Some(app) = weak.upgrade() {
                headless::apply_after_layout(&app, &ui);
            }
        });
        drop(received);
    } else {
        // The real meter (meter-runtime), or the simulated fight of --fake.
        let feed: Box<dyn MeterFeed> = match config {
            Some(config) => {
                controller.borrow_mut().journal.item_ids = item_ids(&config);
                updater::watch(|state| {
                    let _ = slint::invoke_from_event_loop(move || controller::dispatch(feed::FeedEvent::Update(state)));
                });
                if let Some(pcap) = &options.replay {
                    let _ = controller.borrow().commands().send(feed::Command::Replay(pcap.clone()));
                }
                Box::new(meter_runtime::Runtime::new(config))
            }
            None => Box::new(fake::FakeFeed { update: options.update || options.screenshots.is_some(), autostart: !options.idle }),
        };
        let mailbox = std::sync::Arc::new(mailbox::Mailbox::default());
        feed_thread = Some(
            std::thread::Builder::new()
                .name("feed".into())
                .spawn(move || feed.run(Box::new(move |event| mailbox.post(event, controller::dispatch)), received))
                .expect("the feed thread starts"),
        );
    }
    controller::install(controller.clone());

    // The maximized state can change from the window manager (a keyboard shortcut, a snap): read again on each resize,
    // so nothing polls while idle.
    {
        use slint::winit_030::{EventResult, WinitWindowAccessor, winit::event::WindowEvent};
        let weak = Rc::downgrade(&controller);
        app.window().on_winit_window_event(move |_, event| {
            if matches!(event, WindowEvent::Resized(_))
                && let Some(controller) = weak.upgrade()
                && let Ok(controller) = controller.try_borrow()
            {
                controller.poll_window();
            }
            EventResult::Propagate
        });
    }

    if let Some(dir) = options.screenshots {
        screenshots::schedule(&app, dir);
    }
    if let Err(error) = app.run() {
        fall_back(&renderer, &error);
    }
    controller.borrow().save_placement();
    // Closing: the command channel closes with the controller, and the meter saves the record, ends the AlbionLab
    // session and stops the capture before its thread ends.
    controller::uninstall();
    drop(controller);
    drop(app);
    if let Some(thread) = feed_thread {
        let _ = thread.join();
    }
    drop(_instance);
    desktop::restart_if_asked();
    updater::apply_on_exit();
    Ok(())
}

/// The item ids of the item indexes, from the shipped combat.json (for item icons in the journal).
fn item_ids(config: &meter_runtime::Config) -> journal::ItemIds {
    let catalog = config.assets.as_ref().and_then(|assets| meter_core::CombatSourceCatalog::load(assets.join("combat.json")));
    Rc::new(move |index| catalog.as_ref().and_then(|catalog| catalog.find_item(index)).map(|item| std::sync::Arc::from(item.id.as_str())))
}
