//! `--screen <name> --screenshot <png>`: renders one fixture screen with the software renderer into a PNG of the exact
//! window size, without a window or an event loop (for rust/scripts/ui-parity.mjs).

use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use slint::ComponentHandle;
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};

use crate::controller::Controller;
use crate::fixture;
use crate::i18n::Language;
use crate::{AppWindow, CatalogState, JournalState, MeterState, Shell, Theme};

pub(crate) struct Headless {
    pub window: Rc<MinimalSoftwareWindow>,
    pub start: Instant,
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> std::time::Duration {
        self.start.elapsed()
    }
}

/// The state the React side clicks into before its screenshot.
pub fn apply(app: &AppWindow, ui: &fixture::Ui) {
    app.global::<Shell>().set_view(ui.view.as_str().into());
    if ui.view == "journal" {
        app.global::<Shell>().set_journal_enabled(true);
        let journal = app.global::<JournalState>();
        journal.invoke_metric_changed(ui.metric.as_str().into());
        journal.invoke_grouping_changed(ui.grouping.as_str().into());
        if !ui.range.is_empty() {
            journal.invoke_range_changed(ui.range.as_str().into());
        }
        if ui.floor > 0 {
            journal.invoke_select_floor(ui.floor as i32);
        }
    } else {
        app.global::<MeterState>().invoke_metric_changed(ui.metric.as_str().into());
    }
    app.global::<MeterState>().set_confirm(ui.confirm.as_str().into());
    let catalog = app.global::<CatalogState>();
    if !ui.tab.is_empty() {
        catalog.invoke_tab_changed(ui.tab.as_str().into());
    }
    if !ui.search.is_empty() {
        catalog.invoke_search_changed(ui.search.as_str().into());
    }
}

/// What needs the first layout: a key on the chart (its size is known by then), the open copy menu, the hovered control.
pub fn apply_after_layout(app: &AppWindow, ui: &fixture::Ui) {
    if !ui.key.is_empty() {
        app.global::<JournalState>().invoke_key(ui.key.as_str().into(), false);
    }
    if ui.scroll > 0.0 {
        app.global::<Shell>().set_scroll_y(-ui.scroll);
    }
    if ui.copy_menu {
        app.global::<MeterState>().set_copy_menu(true);
    }
    #[cfg(feature = "parity")]
    if !ui.hover.is_empty()
        && let Some(element) = i_slint_backend_testing::ElementHandle::find_by_accessible_label(app, &ui.hover).next()
    {
        let (position, size) = (element.absolute_position(), element.size());
        let center = slint::LogicalPosition::new(position.x + size.width / 2.0, position.y + size.height / 2.0);
        app.window().dispatch_event(slint::platform::WindowEvent::PointerMoved { position: center });
    }
}

/// The ranking picture of the screen's snapshot (CopyDamage.tsx damageImage).
fn card(screen: fixture::Screen, output: &Path) -> Result<(), String> {
    let mut snapshot = None;
    let mut icons = std::collections::HashMap::new();
    for event in screen.events {
        match event {
            crate::feed::FeedEvent::Snapshot(value) => snapshot = Some(value),
            crate::feed::FeedEvent::Icon(id, path) => {
                icons.insert(id, path);
            }
            _ => {}
        }
    }
    let snapshot = snapshot.ok_or("the screen has no snapshot")?;
    let metric = if screen.ui.card == "healing" { crate::ranking::Metric::Healing } else { crate::ranking::Metric::Damage };
    let language = if screen.ui.language == "pt" { Language::Pt } else { Language::En };
    let card = crate::ranking_image::card(&snapshot, metric, language, &icons, crate::desktop::fonts());
    let (width, height, rgba) = crate::ranking_image::render(&card)?;
    std::fs::write(output, crate::screenshots::png(width, height, &rgba)).map_err(|error| format!("{}: {error}", output.display()))
}

/// The real meter offscreen: `meter_runtime::Runtime` with `config`, an optional capture replayed through it, then a
/// screenshot of `view` once the replay ended (or the history loaded) and the meter settled. The record is saved on the
/// way out, as closing the meter does.
pub fn run_runtime(config: meter_runtime::Config, item_ids: crate::journal::ItemIds, replay: Option<std::path::PathBuf>, view: &str, language: Language, output: &Path, size: (u32, u32)) -> Result<(), String> {
    use crate::feed::{Command, FeedEvent, MeterFeed};
    use std::time::Duration;
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless { window: window.clone(), start: Instant::now() })).map_err(|error| error.to_string())?;
    let app = AppWindow::new().map_err(|error| error.to_string())?;
    let (width, height) = size;
    window.set_size(slint::PhysicalSize::new(width, height));
    let (sans, mono) = crate::desktop::fonts();
    app.global::<Theme>().set_sans(sans.into());
    app.global::<Theme>().set_mono(mono.into());
    let (commands, received) = std::sync::mpsc::channel();
    let controller = Controller::new(&app, commands.clone(), language);
    controller.borrow_mut().persist = false;
    controller.borrow_mut().journal.item_ids = item_ids;
    let (events, incoming) = std::sync::mpsc::channel();
    let feed: Box<dyn MeterFeed> = Box::new(meter_runtime::Runtime::new(config));
    let thread = std::thread::spawn(move || feed.run(Box::new(move |event| {
        let _ = events.send(event);
    }), received));
    if let Some(pcap) = &replay {
        let _ = commands.send(Command::Replay(pcap.clone()));
    }
    let deadline = Instant::now() + Duration::from_secs(600);
    let mut settled: Option<Instant> = None;
    let mut dungeon = false;
    let mut failure = None;
    while Instant::now() < deadline && settled.is_none_or(|since| since.elapsed() < Duration::from_millis(1500)) {
        let Ok(event) = incoming.recv_timeout(Duration::from_millis(50)) else {
            if replay.is_none() && settled.is_none() && dungeon {
                settled = Some(Instant::now());
            }
            continue;
        };
        if std::env::var_os("ALBION_METER_TRACE").is_some() {
            match &event {
                FeedEvent::Snapshot(snapshot) => eprintln!("snapshot {} {} {:?} seq {} members {} full {} points {}", snapshot.id, snapshot.started_at, snapshot.mode, snapshot.sequence, snapshot.members.len(), snapshot.full, snapshot.timeline.len()),
                FeedEvent::Capture(status) => eprintln!("capture {} {} {:?}", status.state, status.source, status.error),
                other => eprintln!("{}", format!("{other:?}").chars().take(120).collect::<String>()),
            }
        }
        match &event {
            FeedEvent::CaptureDone(error) if replay.is_some() => {
                failure = error.clone();
                settled = Some(Instant::now());
            }
            FeedEvent::Dungeon(_) => dungeon = true,
            _ => {}
        }
        controller.borrow_mut().handle(event);
    }
    if let Some(error) = failure {
        eprintln!("replay: {error}");
    }
    app.global::<Shell>().set_journal_enabled(view == "journal");
    app.global::<Shell>().set_view(view.into());
    app.show().map_err(|error| error.to_string())?;
    let mut buffer = vec![PremultipliedRgbaColor::default(); (width * height) as usize];
    for _ in 0..4 {
        slint::platform::update_timers_and_animations();
        // Icons and late snapshots that arrived while drawing.
        while let Ok(event) = incoming.try_recv() {
            controller.borrow_mut().handle(event);
        }
        window.request_redraw();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut buffer, width as usize);
        });
        std::thread::sleep(Duration::from_millis(100));
    }
    let rgba: Vec<u8> = buffer.iter().flat_map(|pixel| [pixel.red, pixel.green, pixel.blue, 255]).collect();
    if let Some(parent) = output.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(output, crate::screenshots::png(width, height, &rgba)).map_err(|error| format!("{}: {error}", output.display()))?;
    drop(commands);
    drop(controller);
    let _ = thread.join();
    Ok(())
}

pub fn run(fixture_path: &Path, name: &str, output: &Path) -> Result<(), String> {
    let screen = fixture::screen(fixture_path, name)?;
    if !screen.ui.card.is_empty() {
        return card(screen, output);
    }
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless { window: window.clone(), start: Instant::now() })).map_err(|error| error.to_string())?;
    let app = AppWindow::new().map_err(|error| error.to_string())?;
    let (width, height) = (screen.width as u32, screen.height as u32);
    window.set_size(slint::PhysicalSize::new(width, height));
    let (sans, mono) = crate::desktop::fonts();
    app.global::<Theme>().set_sans(sans.into());
    app.global::<Theme>().set_mono(mono.into());
    let (commands, _received) = std::sync::mpsc::channel();
    let language = if screen.ui.language == "pt" { Language::Pt } else { Language::En };
    let controller = Controller::new(&app, commands, language);
    controller.borrow_mut().persist = false;
    controller.borrow_mut().journal.item_ids = screen.item_ids.clone();
    for event in screen.events {
        controller.borrow_mut().handle(event);
    }
    apply(&app, &screen.ui);
    app.show().map_err(|error| error.to_string())?;
    let mut buffer = vec![PremultipliedRgbaColor::default(); (width * height) as usize];
    let mut draw = |frames: usize| {
        for _ in 0..frames {
            slint::platform::update_timers_and_animations();
            window.request_redraw();
            window.draw_if_needed(|renderer| {
                renderer.render(&mut buffer, width as usize);
            });
        }
    };
    // Bindings that depend on the first layout (the window width, the chart size) settle over a few frames.
    draw(3);
    apply_after_layout(&app, &screen.ui);
    draw(3);
    let rgba: Vec<u8> = buffer.iter().flat_map(|pixel| [pixel.red, pixel.green, pixel.blue, 255]).collect();
    if let Some(parent) = output.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(output, crate::screenshots::png(width, height, &rgba)).map_err(|error| format!("{}: {error}", output.display()))?;
    drop(controller);
    Ok(())
}
