//! Updates with Velopack, as electron-updater did in desktop/main.js: a check at start and every six hours, the download
//! with its progress, "ready" in the update toast, and the install either when the player restarts from the toast or
//! when the meter closes. Only an installed meter updates (Velopack says so; a development build does not).
//!
//! The feed is the `rust-latest` GitHub release that .github/workflows/rust.yml keeps up to date with the newest
//! version's packages and `releases.<channel>.json` (Velopack's latest.yml). It is read from that fixed address rather
//! than by listing the repository's releases: Velopack's GitHub source only looks at the ten newest, and the Electron
//! meter's releases would push the Rust ones out of them. The channel is the one the meter was packaged with
//! (`rust-win` or `rust-linux`), so each system gets its own packages.

use std::sync::Mutex;
use std::time::Duration;

use velopack::sources::HttpSource;
use velopack::{UpdateCheck, UpdateInfo, UpdateManager, UpdateOptions};

use crate::feed::UpdateState;

pub const FEED: &str = "https://github.com/AlbionLab/AlbionMeter/releases/download/rust-latest";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// The downloaded update, and whether the player asked to restart into it.
struct Pending {
    manager: UpdateManager,
    update: Option<UpdateInfo>,
    restart: bool,
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

fn pending() -> std::sync::MutexGuard<'static, Option<Pending>> {
    PENDING.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Velopack's install, update and uninstall hooks; must run first thing in `main` (it may end the process).
pub fn hooks() {
    velopack::VelopackApp::build().run();
}

fn manager() -> Option<UpdateManager> {
    UpdateManager::new(HttpSource::new(FEED), Some(UpdateOptions::default()), None).ok()
}

/// Starts watching for updates when this meter was installed by Velopack; `publish` receives the toast states.
pub fn watch(publish: impl Fn(UpdateState) + Send + Sync + 'static) {
    let Some(manager) = manager() else { return };
    *pending() = Some(Pending { manager: manager.clone(), update: None, restart: false });
    let _ = std::thread::Builder::new().name("updates".into()).spawn(move || {
        loop {
            check(&manager, &publish);
            std::thread::sleep(CHECK_EVERY);
        }
    });
}

fn check(manager: &UpdateManager, publish: &(impl Fn(UpdateState) + Sync)) {
    if pending().as_ref().is_some_and(|pending| pending.update.is_some()) {
        return;
    }
    publish(UpdateState::Checking);
    let update = match manager.check_for_updates() {
        Ok(UpdateCheck::UpdateAvailable(update)) => *update,
        Ok(_) => return publish(UpdateState::Current),
        Err(error) => {
            eprintln!("update check failed: {error}");
            return publish(UpdateState::Current);
        }
    };
    let version = update.TargetFullRelease.Version.clone();
    let total = Some(update.TargetFullRelease.Size).filter(|size| *size > 0);
    publish(UpdateState::Downloading { version: version.clone(), percent: 0, transferred: Some(0), total });
    let (progress, percents) = std::sync::mpsc::channel::<i16>();
    let download = std::thread::scope(|scope| {
        let reporter = scope.spawn(|| {
            for percent in percents {
                let percent = percent.clamp(0, 100) as u8;
                let transferred = total.map(|total| total * percent as u64 / 100);
                publish(UpdateState::Downloading { version: version.clone(), percent, transferred, total });
            }
        });
        let result = manager.download_updates(&update, Some(progress));
        let _ = reporter.join();
        result
    });
    match download {
        Ok(()) => {
            if let Some(pending) = pending().as_mut() {
                pending.update = Some(update);
            }
            publish(UpdateState::Ready { version });
        }
        Err(error) => {
            eprintln!("update failed: {error}");
            publish(UpdateState::Error("The update could not be downloaded. It will be tried again later.".into()));
        }
    }
}

/// "Restart and update": the meter closes as usual (the record is saved) and `apply_on_exit` restarts it on the new
/// version. Returns false when no update is ready.
pub fn restart_into_update() -> bool {
    let mut pending = pending();
    match pending.as_mut() {
        Some(pending) if pending.update.is_some() => {
            pending.restart = true;
            let _ = slint::quit_event_loop();
            true
        }
        _ => false,
    }
}

/// Called last in `main`: a downloaded update is applied once this process has exited, restarting the meter only when
/// the player asked for it (autoInstallOnAppQuit / quitAndInstall).
pub fn apply_on_exit() {
    let Some(pending) = pending().take() else { return };
    let Some(update) = pending.update else { return };
    let arguments: [&str; 0] = [];
    if let Err(error) = pending.manager.wait_exit_then_apply_updates(&update, true, pending.restart, arguments) {
        eprintln!("the update could not be installed: {error}");
    }
}
