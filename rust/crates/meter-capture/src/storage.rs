//! `CaptureStorage`: recordings older builds kept next to the history; removed when debug capture is off.

use std::fs;
use std::path::Path;

/// Deletes `<guid>.pcap` recordings (with their `.pcap.json` reports) and `record-controls.jsonl`, returning the
/// bytes freed. Files that cannot be removed are left alone.
pub fn remove_legacy_recordings(directory: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(directory) else {
        return 0;
    };
    let mut removed = 0;
    for path in entries.flatten().map(|entry| entry.path()) {
        let is_recording = path
            .extension()
            .is_some_and(|extension| extension == "pcap")
            && path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(is_guid_n)
            && path.is_file();
        if !is_recording {
            continue;
        }
        let Some(length) = remove(&path) else {
            continue;
        };
        removed += length;
        let mut report = path.into_os_string();
        report.push(".json");
        removed += remove(Path::new(&report)).unwrap_or(0);
    }
    removed + remove(&directory.join("record-controls.jsonl")).unwrap_or(0)
}

fn remove(path: &Path) -> Option<u64> {
    let length = fs::metadata(path).ok()?.len();
    fs::remove_file(path).ok().map(|()| length)
}

/// `Guid.TryParseExact(value, "N")`: 32 hex digits, no dashes.
fn is_guid_n(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    pub(crate) fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("meter-capture-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn removes_only_guid_recordings_and_controls() {
        let directory = scratch("storage");
        let guid = "0123456789abcdefABCDEF0123456789";
        for (name, size) in [
            (format!("{guid}.pcap"), 100),
            (format!("{guid}.pcap.json"), 10),
            ("record-controls.jsonl".into(), 5),
            ("my-capture.pcap".into(), 7),
            ("0123456789abcdef0123456789abcdef.pcapng".into(), 3),
            ("01234567-89ab-cdef-0123-456789abcdef.pcap".into(), 3),
            ("history.db".into(), 9),
        ] {
            fs::write(directory.join(name), vec![0u8; size]).unwrap();
        }
        assert_eq!(remove_legacy_recordings(&directory), 115);
        let mut left: Vec<_> = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "01234567-89ab-cdef-0123-456789abcdef.pcap",
                "0123456789abcdef0123456789abcdef.pcapng",
                "history.db",
                "my-capture.pcap"
            ]
        );
        assert_eq!(remove_legacy_recordings(&directory.join("missing")), 0);
        fs::remove_dir_all(directory).unwrap();
    }
}
