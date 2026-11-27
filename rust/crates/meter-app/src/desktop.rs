//! desktop/main.js: the settings file, the window placement, fonts, links, file pickers and the Linux capture permission.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::i18n::Language;

/// Electron's userData folder for "Albion Meter", so the language and placement chosen there carry over.
/// `ALBION_METER_CONFIG_DIR` replaces it (tests and screenshot runs never touch the real one).
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("ALBION_METER_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    };
    base.map(|base| base.join("Albion Meter"))
}

fn read_json(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

fn write_json(path: &Path, changes: serde_json::Map<String, serde_json::Value>) {
    let mut value = read_json(path);
    value.extend(changes);
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
    if let Ok(text) = serde_json::to_string_pretty(&value) {
        let _ = std::fs::write(path, text);
    }
}

/// The renderer: `ALBION_METER_RENDERER`, else `renderer` in settings.json, else software. The software renderer is the
/// default: with OpenGL the driver alone maps ~75 MB, over the 60 MB idle budget, and it costs more CPU at 4 Hz.
pub fn renderer() -> String {
    std::env::var("ALBION_METER_RENDERER")
        .ok()
        .or_else(|| config_dir().and_then(|dir| read_json(&dir.join("settings.json")).get("renderer").and_then(|value| value.as_str().map(String::from))))
        .filter(|name| matches!(name.as_str(), "software" | "femtovg"))
        .unwrap_or_else(|| "software".into())
}

/// The chosen language, or the system's on the first run.
pub fn language() -> Language {
    let saved = config_dir().map(|dir| read_json(&dir.join("settings.json"))).and_then(|settings| settings.get("language").and_then(|value| value.as_str().map(String::from)));
    match saved.as_deref() {
        Some("pt") => Language::Pt,
        Some("en") => Language::En,
        _ => Language::system(),
    }
}

pub fn save_language(language: Language) {
    if let Some(dir) = config_dir() {
        write_json(&dir.join("settings.json"), serde_json::Map::from_iter([("language".into(), language.code().into())]));
    }
}

/// The saved size and maximized state of the meter window (window.json, shared with the Electron build).
pub struct Placement {
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

pub fn placement() -> Option<Placement> {
    let saved = read_json(&config_dir()?.join("window.json"));
    let number = |key: &str| saved.get(key).and_then(|value| value.as_f64()).map(|value| value as f32);
    let (width, height) = (number("width")?, number("height")?);
    (width >= 820.0 && height >= 560.0).then(|| Placement { width, height, maximized: saved.get("maximized").and_then(|value| value.as_bool()).unwrap_or(false) })
}

pub fn save_placement(placement: &Placement) {
    if let Some(dir) = config_dir() {
        write_json(
            &dir.join("window.json"),
            serde_json::Map::from_iter([("width".into(), placement.width.into()), ("height".into(), placement.height.into()), ("maximized".into(), placement.maximized.into())]),
        );
    }
}

/// The first installed families of the CSS stacks: Inter / Segoe UI / the system sans, JetBrains Mono / Cascadia Mono /
/// Consolas / the system monospace. An empty name leaves Slint's default.
pub fn fonts() -> (String, String) {
    if cfg!(windows) {
        let fonts = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("Fonts");
        let has = |file: &str| fonts.join(file).exists();
        let sans = if has("Inter.ttf") || has("Inter-Regular.ttf") { "Inter" } else { "Segoe UI" };
        let mono = if has("JetBrainsMono-Regular.ttf") { "JetBrains Mono" } else if has("CascadiaMono.ttf") { "Cascadia Mono" } else { "Consolas" };
        return (sans.into(), mono.into());
    }
    let mut found = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    for dir in [PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts"), home.join(".local/share/fonts"), home.join(".fonts"), PathBuf::from("/Library/Fonts"), home.join("Library/Fonts")] {
        collect_fonts(&dir, 3, &mut found);
    }
    let has = |prefix: &str| found.iter().any(|name| name.starts_with(prefix));
    let sans = if has("Inter") { "Inter" } else if cfg!(target_os = "macos") { "" } else if has("NotoSans-") { "Noto Sans" } else { "" };
    let mono = if has("JetBrainsMono") {
        "JetBrains Mono"
    } else if cfg!(target_os = "macos") {
        "Menlo"
    } else if has("NotoSansMono-") {
        "Noto Sans Mono"
    } else if has("DejaVuSansMono") {
        "DejaVu Sans Mono"
    } else {
        "monospace"
    };
    (sans.into(), mono.into())
}

fn collect_fonts(dir: &Path, depth: u32, found: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                collect_fonts(&path, depth - 1, found);
            }
        } else if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            found.push(name.to_owned());
        }
    }
}

thread_local! {
    /// Replaces the browser (UI tests).
    pub static OPENER: std::cell::Cell<Option<fn(&str)>> = const { std::cell::Cell::new(None) };
}

/// Opens a link in the default browser.
pub fn open_url(url: &str) {
    if let Some(opener) = OPENER.with(std::cell::Cell::get) {
        return opener(url);
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    let result = if cfg!(windows) {
        Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(error) = result {
        eprintln!("could not open {url}: {error}");
    }
}

pub type Picker = fn(bool, &str, &[&str]) -> Option<PathBuf>;

thread_local! {
    /// Replaces the native picker (UI tests).
    pub static PICKER: std::cell::Cell<Option<Picker>> = const { std::cell::Cell::new(None) };
}

/// A native file or folder picker through the desktop's own dialog tool (zenity or kdialog on Linux). The Windows and
/// macOS pickers come with the integration phase.
pub fn pick(folder: bool, title: &str, filters: &[&str]) -> Option<PathBuf> {
    if let Some(picker) = PICKER.with(std::cell::Cell::get) {
        return picker(folder, title, filters);
    }
    let attempts: Vec<(&str, Vec<String>)> = vec![
        (
            "zenity",
            [vec!["--file-selection".to_owned(), format!("--title={title}")], if folder { vec!["--directory".to_owned()] } else { vec![format!("--file-filter={}", filters.join(" "))] }].concat(),
        ),
        ("kdialog", if folder { vec!["--getexistingdirectory".into(), ".".into()] } else { vec!["--getopenfilename".into(), ".".into(), filters.join(" ")] }),
    ];
    for (program, args) in attempts {
        match Command::new(program).args(&args).output() {
            Ok(output) if output.status.success() => {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                return (!path.is_empty()).then(|| PathBuf::from(path));
            }
            Ok(_) => return None,
            Err(_) => continue,
        }
    }
    None
}

/// Whether this is an installed meter rather than a development build (Electron's `app.isPackaged`):
/// `ALBION_METER_PACKAGED=1/0` decides, else an executable outside a Cargo `target` folder.
pub fn packaged() -> bool {
    match std::env::var("ALBION_METER_PACKAGED").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => std::env::current_exe().is_ok_and(|exe| !exe.components().any(|part| part.as_os_str() == "target")),
    }
}

/// `getcap` reports CAP_NET_RAW on the file.
fn can_capture(executable: &Path) -> bool {
    Command::new("getcap").arg(executable).output().is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("cap_net_raw"))
}

/// The executable that gets the capability. An AppImage runs from a read-only mount, so, like desktop/main.js does with
/// the capture service, the meter (and its `assets`) is copied to `<settings>/server/<version>` once and runs from
/// there; any other install is granted in place.
fn permitted_executable() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    if std::env::var_os("APPIMAGE").is_none() {
        return Some(executable);
    }
    let root = config_dir()?.join("server");
    let version = env!("CARGO_PKG_VERSION");
    let folder = root.join(version);
    let copy = folder.join(executable.file_name()?);
    if !copy.exists() {
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).ok()?;
        std::fs::copy(&executable, &copy).ok()?;
        if let Some(assets) = executable.parent().map(|parent| parent.join("assets")).filter(|assets| assets.is_dir()) {
            copy_tree(&assets, &folder.join("assets"));
        }
    }
    // Older versions' copies go.
    for entry in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        if entry.file_name() != version {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    Some(copy)
}

fn copy_tree(from: &Path, to: &Path) {
    let _ = std::fs::create_dir_all(to);
    for entry in std::fs::read_dir(from).into_iter().flatten().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            let _ = std::fs::copy(entry.path(), target);
        }
    }
}

/// Starts `executable` with this process's arguments and ends this one.
fn hand_over(executable: &Path) -> ! {
    let status = Command::new(executable).args(std::env::args_os().skip(1)).env("ALBION_METER_PERMISSION_ASKED", "1").status();
    std::process::exit(status.ok().and_then(|status| status.code()).unwrap_or(0));
}

/// At start on Linux (desktop/main.js `linuxServer`): an installed meter without CAP_NET_RAW asks for it once through
/// the system's password prompt (pkexec setcap), then runs the permitted executable. Refused, the meter opens anyway and
/// the Capture screen offers "Allow packet capture" again.
pub fn capture_permission_at_start() {
    if !cfg!(target_os = "linux") || !packaged() || std::env::var_os("ALBION_METER_PERMISSION_ASKED").is_some() {
        return;
    }
    let Some(executable) = permitted_executable() else { return };
    let current = std::env::current_exe().ok();
    if can_capture(&executable) {
        if current.as_deref() != Some(executable.as_path()) {
            hand_over(&executable);
        }
        return;
    }
    if Command::new("pkexec").arg("setcap").arg("cap_net_raw=ep").arg(&executable).status().is_ok_and(|status| status.success()) {
        // The capability applies from the next exec, also for this very file.
        hand_over(&executable);
    }
    eprintln!("packet capture permission was not granted; allow it later with: sudo setcap cap_net_raw=ep \"{}\"", executable.display());
}

/// The executable to start once this meter has closed (after a granted permission), so the capability applies.
pub static RESTART: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// The Capture screen's "Allow packet capture": the same grant. Granted, the meter closes as usual (the record is
/// saved, the single-instance lock released) and `main` starts the permitted executable. Returns false when it was
/// refused or is not needed on this platform.
pub fn grant_capture_permission() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let Some(executable) = permitted_executable() else { return false };
    let granted = Command::new("pkexec").arg("setcap").arg("cap_net_raw=ep").arg(&executable).status().is_ok_and(|status| status.success());
    if granted {
        *RESTART.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(executable);
        let _ = slint::quit_event_loop();
    }
    granted
}

/// Starts the executable a granted permission asked for, once this meter is closed.
pub fn restart_if_asked() {
    if let Some(executable) = RESTART.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() {
        let _ = Command::new(executable).args(std::env::args_os().skip(1)).env("ALBION_METER_PERMISSION_ASKED", "1").spawn();
    }
}

/// On Windows the meter needs an elevated token for raw-socket capture (the Electron build asked for it in its
/// manifest; the C# server restarted itself elevated): an unelevated start goes through the UAC prompt once.
pub fn elevate_on_windows() {
    #[cfg(windows)]
    if packaged() && std::env::var_os("ALBION_METER_NO_ELEVATION").is_none() && !meter_capture::elevation::is_administrator() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if meter_capture::elevation::restart_elevated(&args) {
            std::process::exit(0);
        }
        eprintln!("running without administrator rights: packet capture will not start");
    }
}
