//! Persistent per-file scroll position store.
//!
//! Positions live in a single JSON file under `$XDG_STATE_HOME/tootles/` (falling
//! back to `~/.local/state/tootles/`). Files are keyed by the SHA-256 of their
//! canonical absolute path; this avoids storing the raw path on disk for
//! privacy and keeps keys fixed-width.
//!
//! Writes are atomic: we write to a tempfile in the same directory, then
//! rename it over the target. On Linux, a same-directory rename is atomic,
//! so a crash mid-write cannot corrupt the file.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const STATE_FILE: &str = "positions.json";
const VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
struct Store {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    positions: BTreeMap<String, Entry>,
    /// Last theme the user was reading with. None = never set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    theme: Option<String>,
    /// Last typographic layout the user was reading with. None = never set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    layout: Option<String>,
    /// Last column alignment the user was reading with. None = never set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    align: Option<String>,
    /// Last code-wrap preference (None = never set = default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wrap_code: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub struct Entry {
    pub offset: usize,
    pub updated_at_unix: u64,
}

pub struct PositionStore {
    path: PathBuf,
    inner: Store,
    enabled: bool,
}

impl PositionStore {
    /// Open (creating if needed) the position store.
    pub fn open() -> Result<Self> {
        let dir = state_dir()?;
        fs::create_dir_all(&dir).with_context(|| format!("creating state dir {}", dir.display()))?;
        let path = dir.join(STATE_FILE);
        let inner = match fs::read_to_string(&path) {
            Ok(data) => serde_json::from_str::<Store>(&data).unwrap_or_default(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Store::default(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Ok(Self {
            path,
            inner,
            enabled: true,
        })
    }

    /// A disabled store satisfies the same API but persists nothing.
    pub fn disabled() -> Self {
        Self {
            path: PathBuf::new(),
            inner: Store::default(),
            enabled: false,
        }
    }

    pub fn get(&self, file: &Path) -> Option<Entry> {
        let key = key_for(file)?;
        self.inner.positions.get(&key).copied()
    }

    pub fn set(&mut self, file: &Path, offset: usize) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let Some(key) = key_for(file) else { return Ok(()); };
        let entry = Entry {
            offset,
            updated_at_unix: now_unix(),
        };
        self.inner.positions.insert(key, entry);
        self.prune();
        self.flush()
    }

    /// The last theme name the user was reading with, if any has been stored.
    pub fn theme(&self) -> Option<&str> {
        self.inner.theme.as_deref()
    }

    /// Persist the given theme name as the "last theme". Silently no-ops when
    /// the store is disabled (--no-remember).
    pub fn set_theme(&mut self, theme: &str) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.inner.theme.as_deref() == Some(theme) {
            return Ok(());
        }
        self.inner.theme = Some(theme.to_string());
        self.flush()
    }

    pub fn layout(&self) -> Option<&str> {
        self.inner.layout.as_deref()
    }

    pub fn set_layout(&mut self, layout: &str) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.inner.layout.as_deref() == Some(layout) {
            return Ok(());
        }
        self.inner.layout = Some(layout.to_string());
        self.flush()
    }

    pub fn align(&self) -> Option<&str> {
        self.inner.align.as_deref()
    }

    pub fn set_align(&mut self, align: &str) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.inner.align.as_deref() == Some(align) {
            return Ok(());
        }
        self.inner.align = Some(align.to_string());
        self.flush()
    }

    pub fn wrap_code(&self) -> Option<bool> {
        self.inner.wrap_code
    }

    pub fn set_wrap_code(&mut self, wrap: bool) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.inner.wrap_code == Some(wrap) {
            return Ok(());
        }
        self.inner.wrap_code = Some(wrap);
        self.flush()
    }

    fn prune(&mut self) {
        const MAX: usize = 1024;
        if self.inner.positions.len() <= MAX {
            return;
        }
        let mut entries: Vec<(String, Entry)> = self.inner.positions.iter().map(|(k, v)| (k.clone(), *v)).collect();
        entries.sort_by_key(|(_, e)| e.updated_at_unix);
        let drop = entries.len() - MAX;
        for (k, _) in entries.into_iter().take(drop) {
            self.inner.positions.remove(&k);
        }
    }

    fn flush(&mut self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        self.inner.version = VERSION;
        let data = serde_json::to_vec_pretty(&self.inner)?;
        let Some(dir) = self.path.parent() else {
            anyhow::bail!("position store path has no parent");
        };
        let tmp = dir.join(format!(".{STATE_FILE}.tmp"));
        {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp)
                .with_context(|| format!("opening {}", tmp.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = f.set_permissions(fs::Permissions::from_mode(0o600));
            }
            f.write_all(&data)?;
            f.sync_all().ok();
        }
        fs::rename(&tmp, &self.path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), self.path.display()))?;
        Ok(())
    }
}

fn state_dir() -> Result<PathBuf> {
    if let Some(dir) = dirs::state_dir() {
        return Ok(dir.join("tootles"));
    }
    if let Some(home) = dirs::home_dir() {
        return Ok(home.join(".local").join("state").join("tootles"));
    }
    anyhow::bail!("could not determine a state directory for your platform");
}

fn key_for(path: &Path) -> Option<String> {
    let canonical = fs::canonicalize(path).ok()?;
    let s = canonical.to_string_lossy();
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    Some(hex_encode(&h.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn disabled_store_noops() {
        let mut s = PositionStore::disabled();
        let tmp = std::env::temp_dir().join("tootles_disabled_test.md");
        File::create(&tmp).unwrap();
        assert!(s.set(&tmp, 42).is_ok());
        assert!(s.get(&tmp).is_none());
    }

    #[test]
    fn hex_encoding_is_stable() {
        assert_eq!(hex_encode(&[0x01, 0xab, 0xff]), "01abff");
    }

    #[test]
    fn theme_serializes_and_deserializes() {
        let s = Store {
            theme: Some("sepia".to_string()),
            ..Store::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Store = serde_json::from_str(&json).unwrap();
        assert_eq!(back.theme.as_deref(), Some("sepia"));
    }

    #[test]
    fn missing_theme_field_defaults_to_none() {
        let json = r#"{"version":1,"positions":{}}"#;
        let s: Store = serde_json::from_str(json).unwrap();
        assert!(s.theme.is_none());
    }

    #[test]
    fn disabled_store_set_theme_noops() {
        let mut s = PositionStore::disabled();
        assert!(s.set_theme("sepia").is_ok());
        assert!(s.theme().is_none());
    }
}
