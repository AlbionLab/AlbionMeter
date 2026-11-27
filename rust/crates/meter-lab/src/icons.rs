//! WeaponIconCache.cs: weapon and item icons from the render service, downloaded once per id and kept in
//! `assets/weapon-icons`.

use meter_store::paths;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::OnceCell;

pub const RENDER_SERVICE: &str = "https://render.albiononline.com";
const SIZES: [&str; 3] = ["?size=128", "", "?size=64"];
const MAX_BYTES: usize = 256_000;
const PNG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
const TIMEOUT: Duration = Duration::from_secs(8);

pub type Icon = Arc<Vec<u8>>;
type Pending = Arc<OnceCell<Option<Icon>>>;

pub struct WeaponIconCache {
    directory: PathBuf,
    service: String,
    http: reqwest::Client,
    known: Box<dyn Fn(&str) -> bool + Send + Sync>,
    pending: Mutex<HashMap<String, Pending>>,
}

impl WeaponIconCache {
    /// `known` says whether an id is an item or weapon of the game data; nothing else is ever requested.
    pub fn new(storage: &Path, known: impl Fn(&str) -> bool + Send + Sync + 'static) -> Self {
        Self::with_service(storage, RENDER_SERVICE, known)
    }

    pub fn with_service(storage: &Path, service: &str, known: impl Fn(&str) -> bool + Send + Sync + 'static) -> Self {
        Self {
            directory: paths::weapon_icons_directory(storage),
            service: service.trim_end_matches('/').to_owned(),
            http: reqwest::Client::builder().timeout(TIMEOUT).build().expect("HTTP client"),
            known: Box::new(known),
            pending: Mutex::default(),
        }
    }

    pub async fn get(&self, id: &str) -> Option<Icon> {
        if id.len() > 150 || !id.chars().all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '@') || !(self.known)(id) {
            return None;
        }
        let request = self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).entry(id.to_owned()).or_default().clone();
        let icon = request.get_or_init(|| self.load(id)).await.clone();
        // Only this request's entry: a newer download of the same icon keeps its own.
        let mut pending = self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if pending.get(id).is_some_and(|current| Arc::ptr_eq(current, &request)) {
            pending.remove(id);
        }
        icon
    }

    async fn load(&self, id: &str) -> Option<Icon> {
        let path = self.directory.join(format!("{id}.png"));
        if let Ok(bytes) = tokio::fs::read(&path).await {
            return Some(Arc::new(bytes));
        }
        // The render service answers 502 for some items at one size and serves them at another, so each is tried in turn.
        for size in SIZES {
            if let Some(bytes) = self.download(id, size).await {
                let (path, bytes) = (path.clone(), Arc::new(bytes));
                let written = bytes.clone();
                tokio::task::spawn_blocking(move || paths::write_atomic(&path, &written)).await.ok()?.ok()?;
                return Some(bytes);
            }
        }
        None
    }

    async fn download(&self, id: &str, size: &str) -> Option<Vec<u8>> {
        let url = format!("{}/v1/item/{}.png{size}", self.service, crate::escape_data_string(id));
        let mut response = self.http.get(url).send().await.ok()?;
        if !response.status().is_success() || response.content_length().is_some_and(|length| length as usize > MAX_BYTES) {
            return None;
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            if bytes.len() + chunk.len() > MAX_BYTES {
                return None;
            }
            bytes.extend_from_slice(&chunk);
        }
        bytes.starts_with(&PNG).then_some(bytes)
    }
}
