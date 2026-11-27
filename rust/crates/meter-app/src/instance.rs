//! One meter per storage folder, as Electron's `requestSingleInstanceLock` (and the C# server's port 5187) made sure.
//! The first instance holds a lock on `meter.lock` and listens on a loopback port written next to it; a second one
//! connects there, asks the first to come to the front and quits.

use std::fs::File;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::Path;
use std::time::Duration;

pub enum Instance {
    /// This is the meter; keep the guard alive for as long as it runs.
    First(Guard),
    /// Another meter runs on this storage folder and was asked to show itself.
    Second,
}

pub struct Guard {
    _lock: File,
}

/// Claims `folder`; `focus` runs (on a background thread) each time a second instance asks the first to show itself.
pub fn claim(folder: &Path, focus: impl Fn() + Send + 'static) -> Instance {
    let _ = std::fs::create_dir_all(folder);
    let port_file = folder.join("meter.port");
    let lock = match File::options().create(true).truncate(false).write(true).open(folder.join("meter.lock")) {
        Ok(lock) => lock,
        // Without a lock file (a read-only folder) there is nothing to coordinate with.
        Err(_) => return Instance::First(Guard { _lock: tempfile_lock() }),
    };
    if lock.try_lock().is_err() {
        if let Some(port) = std::fs::read_to_string(&port_file).ok().and_then(|text| text.trim().parse::<u16>().ok())
            && let Ok(mut stream) = TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, port).into(), Duration::from_secs(2))
        {
            let _ = stream.write_all(b"focus\n");
        }
        return Instance::Second;
    }
    if let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        && let Ok(address) = listener.local_addr()
    {
        let _ = std::fs::write(&port_file, address.port().to_string());
        let _ = std::thread::Builder::new().name("instance".into()).spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = [0u8; 6];
                let mut stream = stream;
                let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                if stream.read_exact(&mut line).is_ok() && &line == b"focus\n" {
                    focus();
                }
            }
        });
    }
    Instance::First(Guard { _lock: lock })
}

fn tempfile_lock() -> File {
    File::open(std::env::temp_dir()).unwrap_or_else(|_| File::open(".").expect("the current folder opens"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn a_second_instance_asks_the_first_to_show_itself() {
        let folder = std::env::temp_dir().join(format!("albion-meter-instance-{}", std::process::id()));
        let focused = Arc::new(AtomicUsize::new(0));
        let seen = focused.clone();
        let first = claim(&folder, move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        assert!(matches!(first, Instance::First(_)));
        assert!(matches!(claim(&folder, || {}), Instance::Second));
        for _ in 0..50 {
            if focused.load(Ordering::SeqCst) == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(focused.load(Ordering::SeqCst), 1);
        drop(first);
        // Once the first is gone the folder is free again.
        assert!(matches!(claim(&folder, || {}), Instance::First(_)));
        let _ = std::fs::remove_dir_all(folder);
    }
}
