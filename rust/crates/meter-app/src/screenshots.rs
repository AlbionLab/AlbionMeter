//! `--screenshots <dir>`: walks through every screen and state on the fake feed and saves each as a PNG, for comparing
//! with docs/images and the Electron build.

use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use slint::ComponentHandle;

use crate::{AppWindow, JournalState, LabState, MeterState, Shell, T};

type Step = (&'static str, fn(&AppWindow));

const STEPS: &[Step] = &[
    ("meter-damage", |_| {}),
    ("meter-healing", |app| app.global::<MeterState>().invoke_metric_changed("healing".into())),
    ("meter-pt", |app| {
        app.global::<MeterState>().invoke_metric_changed("damage".into());
        app.global::<Shell>().invoke_set_language(true);
    }),
    ("meter-confirm-end", |app| {
        app.global::<Shell>().invoke_set_language(false);
        app.global::<MeterState>().set_confirm("end".into());
    }),
    ("meter-confirm-reset", |app| app.global::<MeterState>().set_confirm("reset".into())),
    ("meter-paused", |app| {
        app.global::<MeterState>().set_confirm("".into());
        app.global::<MeterState>().invoke_control("pause".into());
    }),
    ("meter-ended", |app| {
        app.global::<MeterState>().invoke_confirmed("end".into());
    }),
    ("lab-disconnected", |app| app.global::<MeterState>().invoke_confirmed("disconnect".into())),
    ("lab-pairing", |app| app.global::<LabState>().invoke_connect()),
    ("capture", |app| app.global::<Shell>().set_view("capture".into())),
    ("game-data", |app| app.global::<Shell>().set_view("game".into())),
    ("journal", |app| app.global::<Shell>().set_view("journal".into())),
    ("journal-items", |app| {
        let journal = app.global::<JournalState>();
        journal.invoke_grouping_changed("item".into());
        journal.invoke_select_floor(1);
    }),
    ("meter-pt-capture", |app| {
        app.global::<Shell>().invoke_set_language(true);
        app.global::<Shell>().set_view("capture".into());
    }),
    ("game-data-pt", |app| app.global::<Shell>().set_view("game".into())),
];

pub fn schedule(app: &AppWindow, dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    let timer = Rc::new(slint::Timer::default());
    let weak = app.as_weak();
    let step = std::cell::Cell::new(0usize);
    let keep = timer.clone();
    // The first frame shows the loading screen; then each step gets a second to settle (the fake feed ticks 4×/s).
    let (early, folder) = (weak.clone(), dir.clone());
    slint::Timer::single_shot(Duration::from_millis(350), move || {
        if let Some(app) = early.upgrade() {
            save(&app, &folder, "loading");
        }
    });
    let started = std::time::Instant::now();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(1500), move || {
        let _ = &keep;
        let Some(app) = weak.upgrade() else { return };
        // Wait for some fight to have happened, and for the update toast of the fake feed.
        if started.elapsed() < Duration::from_secs(24) {
            return;
        }
        let index = step.get();
        if index > 0 {
            save(&app, &dir, STEPS[index - 1].0);
        }
        if index == STEPS.len() {
            app.global::<T>().set_pt(false);
            let _ = slint::quit_event_loop();
            return;
        }
        (STEPS[index].1)(&app);
        step.set(index + 1);
    });
}

fn save(app: &AppWindow, dir: &std::path::Path, name: &str) {
    match app.window().take_snapshot() {
        Ok(buffer) => {
            let path = dir.join(format!("{name}.png"));
            if let Err(error) = std::fs::File::create(&path).and_then(|mut file| file.write_all(&png(buffer.width(), buffer.height(), buffer.as_bytes()))) {
                eprintln!("{}: {error}", path.display());
            }
        }
        Err(error) => eprintln!("snapshot {name}: {error}"),
    }
}

/// A PNG with stored (uncompressed) deflate blocks: no encoder dependency for a developer flag.
pub fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    fn crc(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for byte in bytes {
            crc ^= *byte as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend(kind);
        out.extend(data);
        let sum = crc(&out[start..]);
        out.extend(sum.to_be_bytes());
    }
    let mut raw = Vec::with_capacity((width as usize * 4 + 1) * height as usize);
    for row in rgba.chunks(width as usize * 4) {
        raw.push(0);
        raw.extend(row);
    }
    let mut zlib = vec![0x78, 0x01];
    for (index, block) in raw.chunks(65_535).enumerate() {
        zlib.push(u8::from((index + 1) * 65_535 >= raw.len()));
        zlib.extend((block.len() as u16).to_le_bytes());
        zlib.extend((!(block.len() as u16)).to_le_bytes());
        zlib.extend(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &raw {
        a = (a + *byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend(((b << 16) | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend(width.to_be_bytes());
    header.extend(height.to_be_bytes());
    header.extend([8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}
