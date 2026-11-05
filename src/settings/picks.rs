//! Choosing a file without freezing the window. The dialog runs on its own
//! thread; the page notices the answer on a later frame (`take`). While one
//! is open, the phone, Stream Deck and the rest keep working.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum What {
    File,
    Files,
    Folder,
}

#[derive(Default)]
struct State {
    /// Bumped when Settings is opened anew: an answer that arrives for an
    /// older one is dropped.
    generation: u64,
    open: HashSet<String>,
    done: HashMap<String, Vec<PathBuf>>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Settings was opened afresh: forget any answers still waiting.
pub fn reset() {
    if let Ok(mut s) = state().lock() {
        s.generation += 1;
        s.open.clear();
        s.done.clear();
    }
}

/// Open the dialog for `key` (ignored while that one is already open).
/// `filter`: its name and file extensions.
pub fn start(ctx: &eframe::egui::Context, key: &str, what: What, filter: Option<(&str, &[&str])>) {
    let generation = {
        let Ok(mut s) = state().lock() else { return };
        if !s.open.insert(key.to_string()) {
            return;
        }
        s.generation
    };
    let (key, ctx) = (key.to_string(), ctx.clone());
    let filter = filter.map(|(n, e)| (n.to_string(), e.iter().map(|x| x.to_string()).collect::<Vec<_>>()));
    let spawned = std::thread::Builder::new().name("file-dialog".into()).spawn({
        let key = key.clone();
        let ctx = ctx.clone();
        move || {
            let mut d = rfd::AsyncFileDialog::new();
            if let Some((name, ext)) = &filter {
                d = d.add_filter(name, ext);
            }
            let picked: Vec<PathBuf> = pollster::block_on(async {
                match what {
                    What::File => d.pick_file().await.map(|f| vec![f.path().to_path_buf()]),
                    What::Files => d.pick_files().await.map(|v| v.into_iter().map(|f| f.path().to_path_buf()).collect()),
                    What::Folder => d.pick_folder().await.map(|f| vec![f.path().to_path_buf()]),
                }
                .unwrap_or_default()
            });
            if let Ok(mut s) = state().lock() {
                if s.generation == generation {
                    s.open.remove(&key);
                    if !picked.is_empty() {
                        s.done.insert(key, picked);
                    }
                }
            }
            ctx.request_repaint();
        }
    });
    if spawned.is_err() {
        if let Ok(mut s) = state().lock() {
            s.open.remove(&key);
        }
    }
}

/// The files chosen for `key` since the last look, once.
pub fn take(key: &str) -> Option<Vec<PathBuf>> {
    state().lock().ok()?.done.remove(key)
}

/// `take`, for a dialog that picks one file or folder.
pub fn take_one(key: &str) -> Option<PathBuf> {
    take(key).and_then(|v| v.into_iter().next())
}
