//! Albion Meter: one native process with a Slint UI (see rust/README.md).
//!
//! `--fake` runs the whole UI on an animated fake fight until the engine is wired in (`--fake-idle`: waiting for Play). Other flags: `--software` (no
//! OpenGL), `--fake-update` (an update downloads and becomes ready), `--view meter|capture|game|journal`, `--journal`
//! (shows the dungeon journal tab), `--lang en|pt`, `--screenshots <dir>` (saves a PNG of every screen, then quits).

mod controller;
mod desktop;
mod fake;
// The contract with the engine, store and AlbionLab: the integration phase uses what the fake feed does not.
#[allow(dead_code)]
mod feed;
mod fixture;
mod format;
mod headless;
mod i18n;
mod journal;
mod merge;
mod ranking;
mod screenshots;

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
    update: bool,
    journal: bool,
    view: Option<String>,
    language: Option<Language>,
    screenshots: Option<std::path::PathBuf>,
    screen: Option<String>,
    screenshot: Option<std::path::PathBuf>,
    fixture: Option<std::path::PathBuf>,
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
            "--fake-update" => options.update = true,
            "--journal" => options.journal = true,
            "--view" => options.view = args.next(),
            "--lang" => options.language = args.next().map(|value| Language::from_locale(&value)),
            "--screenshots" => options.screenshots = args.next().map(Into::into),
            "--screen" => options.screen = args.next(),
            "--screenshot" => options.screenshot = args.next().map(Into::into),
            "--fixture" => options.fixture = args.next().map(Into::into),
            _ => eprintln!("unknown option {arg}"),
        }
    }
    options
}

/// femtovg (OpenGL) first; the software renderer when there is no usable OpenGL driver or when asked for.
fn select_backend(software: bool) {
    let selector = |renderer: &str| slint::BackendSelector::new().backend_name("winit".into()).renderer_name(renderer.into()).select();
    if software || std::env::var_os("SLINT_BACKEND").is_some() {
        if std::env::var_os("SLINT_BACKEND").is_none() {
            let _ = selector("software");
        }
        return;
    }
    if let Err(error) = selector("femtovg") {
        eprintln!("OpenGL renderer unavailable ({error}); using the software renderer");
        let _ = selector("software");
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let options = options();
    let fixture_path = options.fixture.clone().unwrap_or_else(fixture::default_path);
    if let (Some(screen), Some(output)) = (&options.screen, &options.screenshot) {
        if let Err(error) = headless::run(&fixture_path, screen, output) {
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
    select_backend(options.software);
    let app = AppWindow::new()?;
    let (sans, mono) = desktop::fonts();
    app.global::<Theme>().set_sans(sans.into());
    app.global::<Theme>().set_mono(mono.into());
    if let Some(placement) = desktop::placement() {
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

    if let Some(screen) = screen {
        // One fixture screen in a real window: its events, then the UI state the React side clicks into.
        for event in screen.events {
            controller.borrow_mut().handle(event);
        }
        shell.set_view(screen.ui.view.as_str().into());
        app.global::<MeterState>().invoke_metric_changed(screen.ui.metric.as_str().into());
        app.global::<MeterState>().set_confirm(screen.ui.confirm.as_str().into());
        drop(received);
    } else if options.fake || options.screenshots.is_some() {
        let feed: Box<dyn MeterFeed> = Box::new(fake::FakeFeed { update: options.update || options.screenshots.is_some(), autostart: !options.idle });
        std::thread::Builder::new()
            .name("feed".into())
            .spawn(move || {
                feed.run(
                    Box::new(|event| {
                        let _ = slint::invoke_from_event_loop(move || controller::dispatch(event));
                    }),
                    received,
                )
            })
            .expect("the feed thread starts");
    } else {
        eprintln!("The capture engine is not wired in yet: run with --fake to see the meter on a simulated fight.");
        drop(received);
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
    app.run()?;
    controller.borrow().save_placement();
    Ok(())
}
