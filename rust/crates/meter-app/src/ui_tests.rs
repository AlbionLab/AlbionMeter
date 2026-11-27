//! The UI driven like a user would: Slint's testing backend (no window, mock time), the parity fixture's screens, and
//! clicks, keys and text found by the controls' accessible labels. Each test runs on its own thread with its own
//! backend; nothing touches the real settings (`ALBION_METER_CONFIG_DIR` points at a temporary folder).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use i_slint_backend_testing::{ElementHandle, mock_elapsed_time};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model};

use crate::controller::Controller;
use crate::feed::*;
use crate::i18n::Language;
use crate::ranking::{Metric, player_names, ranking_text};
use crate::{AppWindow, CaptureState, CatalogState, JournalState, LabState, MeterState, Shell, fixture};

struct Ui {
    app: AppWindow,
    controller: Rc<RefCell<Controller>>,
    commands: Receiver<Command>,
}

fn start(screen: &str, language: Language) -> Ui {
    let config = std::env::temp_dir().join(format!("albion-meter-ui-tests-{}", std::process::id()));
    // SAFETY: every test sets the same value; nothing reads it concurrently in a way that matters.
    unsafe { std::env::set_var("ALBION_METER_CONFIG_DIR", &config) };
    thread_local! { static READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
    if !READY.with(|ready| ready.replace(true)) {
        i_slint_backend_testing::init_no_event_loop();
    }
    crate::desktop::OPENER.with(|opener| opener.set(Some(|_| {})));
    let app = AppWindow::new().unwrap();
    app.window().set_size(slint::LogicalSize::new(1280.0, 820.0));
    let (sender, commands) = std::sync::mpsc::channel();
    let controller = Controller::new(&app, sender, language);
    controller.borrow_mut().persist = false;
    let screen = fixture::screen(&fixture::default_path(), screen).unwrap();
    controller.borrow_mut().journal.item_ids = screen.item_ids.clone();
    for event in screen.events {
        controller.borrow_mut().handle(event);
    }
    crate::headless::apply(&app, &screen.ui);
    app.show().unwrap();
    // Loading the fixture asked for its dungeon run; only what the test does counts.
    let _ = commands.try_iter().count();
    Ui { app, controller, commands }
}

impl Ui {
    fn all(&self, label: &str) -> Vec<ElementHandle> {
        ElementHandle::find_by_accessible_label(&self.app, label).collect()
    }

    fn find(&self, label: &str) -> ElementHandle {
        self.all(label).into_iter().next().unwrap_or_else(|| panic!("no control labelled {label:?}"))
    }

    /// A real pointer click at the control's centre (hit-testing decides what receives it).
    fn click(&self, label: &str) {
        self.click_element(&self.find(label));
    }

    /// The control's own action, for controls scrolled out of the 820 px window (what a click would do there).
    fn activate(&self, label: &str) {
        self.find(label).invoke_accessible_default_action();
        mock_elapsed_time(Duration::from_millis(16));
    }

    fn click_element(&self, element: &ElementHandle) {
        element.mock_single_click(PointerEventButton::Left);
        mock_elapsed_time(Duration::from_millis(16));
    }

    fn click_at(&self, x: f32, y: f32) {
        let position = LogicalPosition::new(x, y);
        let window = self.app.window();
        window.dispatch_event(WindowEvent::PointerMoved { position });
        window.dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
        window.dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
        mock_elapsed_time(Duration::from_millis(16));
    }

    fn key(&self, key: Key) {
        let text: slint::SharedString = key.into();
        self.app.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        self.app.window().dispatch_event(WindowEvent::KeyReleased { text });
        mock_elapsed_time(Duration::from_millis(16));
    }

    fn commands(&self) -> Vec<Command> {
        self.commands.try_iter().collect()
    }

    fn snapshot(&self) -> std::sync::Arc<Snapshot> {
        let screen = fixture::screen(&fixture::default_path(), "meter-damage").unwrap();
        screen.events.into_iter().find_map(|event| if let FeedEvent::Snapshot(snapshot) = event { Some(snapshot) } else { None }).unwrap()
    }
}

/// The second click of a double click lands after the browser's double-click time.
fn after_double_click_time() {
    std::thread::sleep(Duration::from_millis(550));
}

#[test]
fn record_controls_send_commands_ignore_double_clicks_and_disable_while_busy() {
    let ui = start("meter-idle", Language::En);
    ui.click("Play");
    assert_eq!(ui.commands(), [Command::StartRecord]);
    // The command is running: the control is disabled and a second click does nothing.
    assert_eq!(ui.find("Play").accessible_enabled(), Some(false));
    ui.click("Play");
    assert!(ui.commands().is_empty());
    let running = ui.snapshot();
    ui.controller.borrow_mut().handle(FeedEvent::Snapshot(running));
    ui.controller.borrow_mut().handle(FeedEvent::CommandDone(None));
    // Pause now sits where Play was: the second click of the double click lands on it and is ignored.
    ui.click("Pause");
    assert!(ui.commands().is_empty());
    after_double_click_time();
    ui.click("Pause");
    assert_eq!(ui.commands(), [Command::PauseRecord]);
    ui.controller.borrow_mut().handle(FeedEvent::CommandDone(None));
    // Not connected: every record control is disabled.
    ui.controller.borrow_mut().handle(FeedEvent::Connection(Connection::Reconnecting));
    assert_eq!(ui.find("Pause").accessible_enabled(), Some(false));
    assert_eq!(ui.find("Reset").accessible_enabled(), Some(false));
    assert_eq!(ui.find("End session").accessible_enabled(), Some(false));
    assert_eq!(ui.app.global::<Shell>().get_connection(), "reconnecting");
}

#[test]
fn reset_and_end_ask_first_and_the_modal_answers_keys_and_the_backdrop() {
    let ui = start("meter-damage", Language::En);
    let meter = ui.app.global::<MeterState>();
    ui.click("Reset");
    assert_eq!(meter.get_confirm(), "reset");
    assert!(ui.commands().is_empty());
    ui.key(Key::Escape);
    assert_eq!(meter.get_confirm(), "");
    after_double_click_time();
    ui.click("Reset");
    ui.key(Key::Return);
    assert_eq!(ui.commands(), [Command::NewRecord]);
    assert_eq!(meter.get_confirm(), "");
    ui.controller.borrow_mut().handle(FeedEvent::CommandDone(None));
    after_double_click_time();
    ui.click("End session");
    assert_eq!(meter.get_confirm(), "end");
    // The backdrop, outside the dialog, cancels.
    ui.click_at(20.0, 800.0);
    assert_eq!(meter.get_confirm(), "");
    after_double_click_time();
    ui.click("End session");
    // The dialog's own End session button (the last one with that label) confirms.
    let buttons = ui.all("End session");
    ui.click_element(buttons.last().unwrap());
    assert_eq!(ui.commands(), [Command::StopRecord]);
}

#[test]
fn copy_menu_copies_the_ranking_and_the_names_and_the_toast_clears() {
    let ui = start("meter-damage", Language::En);
    let meter = ui.app.global::<MeterState>();
    let snapshot = ui.snapshot();
    let copy = ui.find("Copy");
    assert_eq!(copy.accessible_enabled(), Some(true));
    ui.click("Copy");
    assert_eq!(ui.find("Copy").accessible_expanded(), Some(true), "the menu opens");
    ui.click("Ranking as text");
    assert_eq!(ui.app.get_clipboard(), ranking_text(&snapshot, Metric::Damage));
    assert_eq!(meter.get_copy_done(), "text");
    mock_elapsed_time(Duration::from_millis(1700));
    assert_eq!(meter.get_copy_done(), "text");
    mock_elapsed_time(Duration::from_millis(200));
    assert_eq!(meter.get_copy_done(), "", "the confirmation clears after 1.8 s");
    // Healing: the names in the healing order.
    ui.click("Healing");
    ui.click("Copy");
    ui.click("Player names");
    assert_eq!(ui.app.get_clipboard(), player_names(&snapshot, Metric::Healing));
    assert_eq!(meter.get_copy_done(), "names");
    // No players: the button is disabled and opens nothing.
    let ui = start("meter-idle", Language::En);
    assert_eq!(ui.find("Copy").accessible_enabled(), Some(false));
    ui.click("Copy");
    assert_eq!(ui.find("Copy").accessible_expanded(), Some(false));
}

#[test]
fn copy_errors_stay_four_seconds() {
    let ui = start("meter-damage", Language::En);
    let meter = ui.app.global::<MeterState>();
    ui.controller.borrow_mut().image_rendered(Err("boom".into()), crate::controller::CopyTarget::Meter);
    assert_eq!(meter.get_copy_error(), "Image rendering failed.");
    mock_elapsed_time(Duration::from_millis(3900));
    assert_eq!(meter.get_copy_error(), "Image rendering failed.");
    mock_elapsed_time(Duration::from_millis(200));
    assert_eq!(meter.get_copy_error(), "");
}

#[test]
fn albionlab_connect_once_name_editing_and_disconnect() {
    let ui = start("lab-disconnected", Language::En);
    ui.click("Connect account");
    ui.click("Connect account");
    assert_eq!(ui.commands(), [Command::LabConnect], "a double click asks for one pairing code");

    let ui = start("meter-damage", Language::En);
    let lab = ui.app.global::<LabState>();
    let field = ui.find("Session name");
    assert_eq!(field.accessible_placeholder_text().as_deref(), Some("Tumbleweed Hollow · Solo Dungeon"));
    ui.click_element(&field);
    field.set_accessible_value("Boss run");
    ui.key(Key::Return);
    assert_eq!(ui.commands(), [Command::LabTitle("Boss run".into())]);
    // Escape puts the saved name back and saves nothing.
    ui.click_element(&field);
    field.set_accessible_value("Typo");
    ui.key(Key::Escape);
    assert_eq!(field.accessible_value().as_deref(), Some(""));
    assert!(ui.commands().is_empty());
    // The suggestion applies with one click.
    ui.click("Use this name");
    assert_eq!(ui.commands(), [Command::LabTitle("Tumbleweed Hollow · Solo Dungeon".into())]);
    ui.controller.borrow_mut().handle(FeedEvent::LabError("AlbionLab answered 503.".into()));
    assert_eq!(lab.get_error(), "AlbionLab answered 503.");
    ui.click("Hide other players' names");
    assert_eq!(ui.commands(), [Command::LabAnonymize(true)]);
    ui.click("Share dungeon location (map names)");
    assert_eq!(ui.commands(), [Command::LabShareLocation(true)]);
    ui.click("Copy link");
    assert_eq!(ui.app.get_clipboard(), "https://albionlab.app/meter/aBc123XyZ0");
    assert!(lab.get_copied());
    mock_elapsed_time(Duration::from_millis(1600));
    assert!(!lab.get_copied());
    // Disconnect asks first.
    ui.click("Disconnect account");
    assert_eq!(ui.app.global::<MeterState>().get_confirm(), "disconnect");
    assert!(ui.commands().is_empty());
    let buttons = ui.all("Disconnect account");
    ui.click_element(buttons.last().unwrap());
    assert_eq!(ui.commands(), [Command::LabDisconnect]);
}

#[test]
fn messages_from_the_services_are_translated() {
    let ui = start("meter-damage", Language::Pt);
    ui.controller.borrow_mut().handle(FeedEvent::LabError("AlbionLab answered 503.".into()));
    assert_eq!(ui.app.global::<LabState>().get_error(), "O AlbionLab respondeu 503.");
    ui.controller.borrow_mut().handle(FeedEvent::CommandDone(Some("Press Play to start a record first.".into())));
    assert_eq!(ui.app.global::<Shell>().get_error(), "Aperte Play para começar uma gravação primeiro.");
    // "Fechar" is also the window's Close button; the banner's comes after the top bar.
    let dismiss = ui.all("Fechar");
    ui.click_element(dismiss.last().unwrap());
    assert_eq!(ui.app.global::<Shell>().get_error(), "", "Dismiss clears the banner");
    ui.controller.borrow_mut().handle(FeedEvent::CaptureDone(Some("No capture interface is available.".into())));
    assert_eq!(ui.app.global::<CaptureState>().get_error(), "Nenhuma interface de captura disponível.");
}

#[test]
fn capture_notice_pending_players_and_interface_errors() {
    let ui = start("capture-pending", Language::En);
    assert_eq!(ui.app.global::<MeterState>().get_notice_pending(), "45");
    let ui = start("capture-devices-error", Language::En);
    assert_eq!(ui.app.global::<CaptureState>().get_devices_error(), "Packet capture permission denied: run Albion Meter as administrator.");
    // Windows asks for a restart as administrator; Linux offers the password prompt.
    assert_eq!(ui.app.global::<CaptureState>().get_hint(), if cfg!(windows) { "windows" } else { "grant" });
    // Start and Stop go to the engine with the chosen interface.
    let ui = start("capture", Language::En);
    ui.controller.borrow_mut().handle(FeedEvent::Capture(std::sync::Arc::new(CaptureStatus { state: "stopped".into(), source: "none".into(), ..CaptureStatus::default() })));
    ui.click("Start capture");
    assert_eq!(ui.commands(), [Command::StartCapture { device: "raw:all".into(), method: "socket".into() }]);
}

#[test]
fn game_data_search_and_folder_import() {
    let ui = start("game-data", Language::En);
    let catalog = ui.app.global::<CatalogState>();
    let search = ui.find("Search game catalog");
    ui.click_element(&search);
    search.set_accessible_value("warbow");
    assert_eq!(catalog.get_results(), "5 results");
    assert_eq!(catalog.get_rows().row_count(), 5);
    search.set_accessible_value("zzz");
    assert!(catalog.get_empty());
    search.set_accessible_value("");
    assert_eq!(catalog.get_results(), "32 results");
    ui.click("Maps");
    assert_eq!(catalog.get_results(), "140 results · Showing the first 100, refine your search for more");
    assert_eq!(catalog.get_rows().row_count(), 100);
    crate::desktop::PICKER.with(|picker| picker.set(Some(|_, _, _| Some(PathBuf::from("/games/Albion Online")))));
    ui.activate("Import game folder");
    assert_eq!(ui.commands(), [Command::ImportCatalog(PathBuf::from("/games/Albion Online"))]);
    assert!(catalog.get_busy());
    assert_eq!(catalog.get_message(), "Reading selected game files…");
    ui.controller.borrow_mut().handle(FeedEvent::CatalogMessage("imported:140:32".into()));
    assert!(!catalog.get_busy());
    assert_eq!(catalog.get_message(), "Imported 140 maps and 32 weapon definitions.");
}

#[test]
fn header_views_language_and_window_actions() {
    let ui = start("meter-damage", Language::En);
    ui.click("Capture");
    assert_eq!(ui.app.global::<Shell>().get_view(), "capture");
    ui.click("Game data");
    assert_eq!(ui.app.global::<Shell>().get_view(), "game");
    ui.click("PT");
    assert_eq!(ui.find("Dados do jogo").accessible_checked(), Some(true));
    ui.click("EN");
    // An empty spot of the bar moves the window; a double click maximizes it; the buttons do what they say.
    ui.click_at(400.0, 26.0);
    // The window's edges resize it (5 px grips).
    ui.click_at(640.0, 818.0);
    ui.click_at(1278.0, 400.0);
    ui.click_at(2.0, 2.0);
    ui.click("Minimize");
    ui.click("Maximize");
    ui.click("Close");
    let actions = ui.controller.borrow().window_actions.clone();
    assert_eq!(actions, ["drag", "resize-s", "resize-e", "resize-nw", "minimize", "maximize", "close"]);
}

#[test]
fn dungeon_journal_interactions() {
    let ui = start("journal", Language::En);
    let journal = ui.app.global::<JournalState>();
    assert_eq!(journal.get_runs().row_count(), 1);
    assert!(journal.get_has_run());
    assert_eq!(journal.get_floors().row_count(), 4);
    assert_eq!(journal.get_damage(), "256.6K");
    // Floor tabs
    ui.click("Floor 2");
    assert_eq!(journal.get_chart_title(), "Floor 2");
    assert_eq!(journal.get_chart_note(), "Sunken Crypt");
    assert!(!journal.get_damage_note().is_empty());
    ui.click("All floors");
    assert_eq!(journal.get_chart_title(), "Entire dungeon");
    // Timeline: keys pin an interval, Escape follows the latest again, a click picks the second under the pointer.
    journal.invoke_range_changed("0".into());
    journal.invoke_key("Home".into(), false);
    assert_eq!(journal.get_interval(), "00:00 – 00:01");
    journal.invoke_key("ArrowRight".into(), true);
    assert_eq!(journal.get_interval(), "00:10 – 00:11");
    journal.invoke_key("ArrowLeft".into(), false);
    assert_eq!(journal.get_interval(), "00:09 – 00:10");
    journal.invoke_key("End".into(), false);
    assert_eq!(journal.get_interval(), "05:00 – 05:01");
    journal.invoke_key("Escape".into(), false);
    assert!(journal.get_cursor_shown());
    journal.invoke_resolution_changed("10".into());
    journal.invoke_key("Home".into(), false);
    assert_eq!(journal.get_interval(), "00:00 – 00:10");
    // Two minutes of five: the scrubber shows, Latest follows the end again.
    journal.invoke_range_changed("120".into());
    assert!(journal.get_scrubber());
    journal.invoke_scrub(0.0);
    assert!(!journal.get_latest());
    journal.invoke_follow_latest();
    assert!(journal.get_latest());
    // Inspector controls
    journal.invoke_scope_changed("record".into());
    assert!(journal.get_summary_total().ends_with(" damage"));
    journal.invoke_order_changed("name".into());
    assert_eq!(journal.get_inspector().row_data(0).unwrap().name, "Edsonjds");
    journal.invoke_inspector_search_changed("kp".into());
    assert_eq!(journal.get_inspector().row_count(), 1);
    journal.invoke_toggle_player("d4".into());
    let player = journal.get_inspector().row_data(0).unwrap();
    assert!(player.open);
    assert!(player.items.row_count() > 0);
    journal.invoke_inspector_search_changed("".into());
    // Players & equipment: four players start open, Collapse all closes them.
    let rows = journal.get_loadout().row_count();
    assert!(rows > 4);
    // (The element query does not descend past the inspector's scroll area, so the table's toggle is driven by its callback.)
    journal.invoke_toggle_loadout_all();
    assert_eq!(journal.get_loadout().row_count(), 4);
    journal.invoke_toggle_loadout("d1".into());
    assert!(journal.get_loadout().row_count() > 4);
    let progression = journal.get_progression().row_count();
    journal.invoke_toggle_progression_all();
    assert_eq!(journal.get_progression().row_count(), 4);
    assert!(progression > 4);
    assert_eq!(journal.get_progression_heads().row_count(), 4);
    assert_eq!(journal.get_floor_rows().row_count(), 3);
    // Grouping by item puts one series per item.
    journal.invoke_grouping_changed("item".into());
    assert!(journal.get_series().row_count() > 4);
    assert!(journal.get_series_legend());
    // Export / import
    ui.activate("Export dungeon");
    let exported = ui.commands();
    assert!(matches!(&exported[..], [Command::OpenDungeon(_), .., Command::ExportHistory { dungeon: Some(id), .. }] | [Command::ExportHistory { dungeon: Some(id), .. }] if &**id == "run-1"), "{exported:?}");
    ui.controller.borrow_mut().handle(FeedEvent::HistoryMessage { text: "Exported".into(), error: false });
    let big = std::env::temp_dir().join(format!("albion-meter-big-{}.zip", std::process::id()));
    std::fs::File::create(&big).unwrap().set_len(41_000_000).unwrap();
    thread_local! { static FILE: RefCell<PathBuf> = RefCell::default(); }
    FILE.with(|file| *file.borrow_mut() = big.clone());
    crate::desktop::PICKER.with(|picker| picker.set(Some(|_, _, _| Some(FILE.with(|file| file.borrow().clone())))));
    ui.activate("Import history");
    assert!(journal.get_transfer_error());
    assert_eq!(journal.get_transfer_message(), "Choose a history file smaller than 40 MB.");
    std::fs::File::create(&big).unwrap().set_len(1_000).unwrap();
    ui.activate("Import history");
    assert_eq!(ui.commands(), [Command::ImportHistory(big.clone())]);
    ui.controller.borrow_mut().handle(FeedEvent::HistoryMessage { text: "Imported 3 encounters and 1 dungeons.".into(), error: false });
    assert_eq!(journal.get_transfer_message(), "Imported 3 encounters and 1 dungeons.");
    let _ = std::fs::remove_file(big);
    // The journal's Copy copies the shown floor's ranking.
    ui.controller.borrow_mut().journal.render();
    ui.find("Copy").invoke_accessible_default_action();
}


