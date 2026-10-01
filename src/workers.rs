//! Background work: a single message bus plus cancellable job bookkeeping.
//!
//! All disk access happens off the UI thread. The UI only ever polls the bus,
//! so the window never blocks on a slow network drive or a huge `copy`.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::fs_model::Entry;

/// A message produced by a worker thread.
pub enum Msg {
    /// A directory listing finished. `token` discards stale replies.
    Listed {
        token: u64,
        path: PathBuf,
        entries: Vec<Entry>,
        error: Option<String>,
    },
    /// A file finished loading into the editor.
    Loaded {
        path: PathBuf,
        /// The document as the reader produced it, flags and all.
        doc: Option<crate::editor::Doc>,
        error: Option<String>,
    },
    /// A filesystem watch event fired somewhere.
    Watch(PathBuf),
    /// The head of a text file, for the details pane.
    Peek {
        path: PathBuf,
        /// `None` when the file could not be read as text.
        text: Option<String>,
    },
    /// A thumbnail finished decoding: raw RGBA, ready for a texture.
    Thumb {
        path: PathBuf,
        /// Requested edge length in pixels, also used as the texture name.
        px: u32,
        /// Empty when decoding failed.
        rgba: Vec<u8>,
        w: u32,
        h: u32,
    },
    /// A folder's subfolders finished loading for the sidebar tree.
    TreeLoaded { path: PathBuf, dirs: Vec<PathBuf> },
    /// A folder's subtree finished being measured for the status bar and the
    /// details pane.
    Measured {
        path: PathBuf,
        measure: crate::ops::Measure,
    },
    /// A file operation reported progress.
    Progress(Progress),
    /// A file operation finished.
    Finished { id: u64, outcome: Outcome },
    /// Recursive search reported results.
    Search(SearchChunk),
}

/// One progress update from a running job.
#[derive(Clone, Debug)]
pub struct Progress {
    pub id: u64,
    pub done_items: usize,
    pub total_items: usize,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub current: String,
    pub failed: Vec<String>,
}

/// Result of a completed job.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// N items processed, M skipped/failed.
    Done { ok: usize, failed: Vec<String> },
    /// Cancelled by the user.
    Cancelled { done: usize },
    /// Aborted before it started, for example when the destination is full.
    #[allow(dead_code)]
    Failed(String),
}

/// Incremental recursive-search results.
#[derive(Clone, Debug)]
pub struct SearchChunk {
    /// Which search sent this, so the answer to one that has been replaced is ignored.
    pub token: u64,
    pub found: Vec<Entry>,
    pub scanned: u64,
    pub done: bool,
    pub truncated: bool,
}

/// Create the channel pair used to talk to workers.
pub fn bus() -> (Sender<Msg>, Receiver<Msg>) {
    std::sync::mpsc::channel()
}

/// Drains every message currently queued without blocking.
pub fn drain(rx: &Receiver<Msg>) -> Vec<Msg> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        out.push(msg);
    }
    out
}

/// A cancellable background job.
pub struct Job {
    pub id: u64,
    pub kind: OpKind,
    pub label: String,
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
    pub started: std::time::Instant,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpKind {
    Copy,
    Move,
    Delete,
    Compress,
    Extract,
}

/// Monotonic id source shared by jobs and requests.
#[derive(Default)]
pub struct Ids(Mutex<u64>);

impl Ids {
    pub fn next(&self) -> u64 {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        *g += 1;
        *g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_monotonic() {
        let ids = Ids::default();
        assert_eq!(ids.next(), 1);
        assert_eq!(ids.next(), 2);
    }

    #[test]
    fn drain_returns_queued_messages_then_stops() {
        let (tx, rx) = bus();
        tx.send(Msg::Progress(Progress {
            id: 1,
            done_items: 0,
            total_items: 1,
            done_bytes: 0,
            total_bytes: 0,
            current: String::new(),
            failed: vec![],
        }))
        .ok();
        let first = drain(&rx);
        assert_eq!(first.len(), 1);
        assert!(drain(&rx).is_empty());
    }
}
