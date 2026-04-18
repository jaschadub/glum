//! File-change watcher for `--follow` mode.
//!
//! Watches the target file's **parent directory** rather than the file
//! itself, because text editors (vim, VS Code, `:w`) typically write via
//! atomic-rename: they create a temporary, write to it, then rename over the
//! original. A watcher attached directly to the original file would lose its
//! subscription the moment the inode is replaced. Watching the parent dir
//! and filtering by filename survives that.
//!
//! Events are de-duplicated: many editors emit two or three rapid events per
//! save (e.g. `Create` + `Modify`). The receiver side in `app.rs` coalesces
//! bursts with a short settle window before triggering a reload.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use anyhow::{Context, Result};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// A live file watcher. The inner `Watcher` is kept alive for the `Receiver`
/// to remain useful; dropping the guard cancels the watch.
pub struct FileWatcher {
    _watcher: RecommendedWatcher,
    rx: Receiver<()>,
}

impl FileWatcher {
    /// Start watching `file`. The directory containing the file is watched;
    /// only events whose path resolves to `file` are forwarded.
    pub fn start(file: &Path) -> Result<Self> {
        let canonical = std::fs::canonicalize(file)
            .with_context(|| format!("canonicalizing {}", file.display()))?;
        let parent = canonical
            .parent()
            .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", canonical.display()))?
            .to_path_buf();
        let target = canonical.clone();

        let (tx, rx) = mpsc::channel::<()>();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else { return };
            if !is_content_change(event.kind) {
                return;
            }
            if !event_touches(&event.paths, &target) {
                return;
            }
            // Ignore send failures — means the reader has exited.
            let _ = tx.send(());
        })
        .context("creating file watcher")?;

        watcher
            .watch(&parent, RecursiveMode::NonRecursive)
            .with_context(|| format!("watching {}", parent.display()))?;

        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    /// Non-blocking: returns `true` if at least one change event is pending.
    /// Drains the channel so bursty editor writes become a single reload.
    pub fn drain(&self) -> bool {
        let mut any = false;
        while self.rx.try_recv().is_ok() {
            any = true;
        }
        any
    }
}

fn is_content_change(kind: EventKind) -> bool {
    // Modify and create (for atomic-rename saves) events indicate the file's
    // content may have changed. Attribute changes, access events, and pure-
    // remove events don't warrant a reload.
    matches!(kind, EventKind::Modify(_) | EventKind::Create(_))
}

fn event_touches(paths: &[PathBuf], target: &Path) -> bool {
    for p in paths {
        // Direct match.
        if p == target {
            return true;
        }
        // Canonical match (handles `./`-prefixed paths from some backends).
        if let Ok(canon) = std::fs::canonicalize(p) {
            if canon == target {
                return true;
            }
        }
        // Parent-dir events where the file name matches.
        if let (Some(ev_name), Some(tgt_name)) = (p.file_name(), target.file_name()) {
            if ev_name == tgt_name {
                return true;
            }
        }
    }
    false
}
