//! desktop/main.js: the settings file, the window placement, fonts, links, file pickers and the Linux capture permission.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::i18n::Language;

/// Electron's userData folder for "Albion Meter", so the language and placement chosen there carry over.
pub fn config_dir() -> Option<PathBuf> {
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

/// Opens a link in the default browser.
pub fn open_url(url: &str) {
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

/// A native file or folder picker through the desktop's own dialog tool (zenity or kdialog on Linux). The Windows and
/// macOS pickers come with the integration phase.
pub fn pick(folder: bool, title: &str, filters: &[&str]) -> Option<PathBuf> {
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

/// Lets this executable read raw packets (CAP_NET_RAW) after the system's password prompt, then starts it again so the
/// capability applies. Returns false when it was refused or is not needed on this platform.
pub fn grant_capture_permission() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let Ok(executable) = std::env::current_exe() else { return false };
    let granted = Command::new("pkexec").arg("setcap").arg("cap_net_raw=ep").arg(&executable).status().is_ok_and(|status| status.success());
    if granted {
        let _ = Command::new(&executable).args(std::env::args_os().skip(1)).spawn();
        std::process::exit(0);
    }
    false
}
