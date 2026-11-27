use std::path::{Path, PathBuf};

pub const DATABASE_FILE: &str = "encounters.db";
pub const ALBIONLAB_FILE: &str = "albionlab.json";
pub const CAPTURE_SETTINGS_FILE: &str = "capture-settings.json";

/// Where the meter keeps its data, as Program.cs decides it: an explicit directory, else the portable `data` folder
/// next to the executable when it exists, else LocalApplicationData/AlbionMeter.
pub fn storage_directory(configured: Option<&Path>) -> PathBuf {
    if let Some(directory) = configured {
        return directory.to_path_buf();
    }
    if let Some(portable) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join("data")))
        && portable.is_dir()
    {
        return portable;
    }
    local_application_data().join("AlbionMeter")
}

/// .NET's `Environment.SpecialFolder.LocalApplicationData`.
pub fn local_application_data() -> PathBuf {
    let home = || std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home().join("AppData").join("Local"))
    } else if cfg!(target_os = "macos") {
        home().join("Library").join("Application Support")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home().join(".local").join("share"))
    }
}

/// `captures` under the storage directory: recordings, local-character.json, local-party.json, known-players.json.
pub fn captures_directory(storage: &Path) -> PathBuf {
    storage.join("captures")
}

/// Weapon and item icons downloaded from the render service.
pub fn weapon_icons_directory(storage: &Path) -> PathBuf {
    storage.join("assets").join("weapon-icons")
}

/// Writes through a `.tmp` sibling and a rename, so a crash never leaves half a file.
pub fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    std::fs::write(&temporary, contents)?;
    std::fs::rename(&temporary, path)
}
