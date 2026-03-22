use crate::state::AppState;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

#[derive(Debug, Clone, Serialize)]
struct FsChangePayload {
    path: String,
    kind: String,
}

fn event_kind_label(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::Create(_) => "create",
        EventKind::Modify(_) => "modify",
        EventKind::Remove(_) => "remove",
        _ => "modify",
    }
}

#[tauri::command]
pub fn fs_watch_dir(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut watchers = state.watchers.lock().map_err(|e| e.to_string())?;

    // Already watching this path
    if watchers.contains_key(&path) {
        return Ok(());
    }

    let watch_path = path.clone();
    let app_handle = app.clone();

    let (tx, rx) = mpsc::channel::<Result<Event, notify::Error>>();

    let mut watcher = RecommendedWatcher::new(tx, Config::default().with_poll_interval(Duration::from_millis(200)))
        .map_err(|e| format!("Failed to create watcher: {e}"))?;

    watcher
        .watch(Path::new(&watch_path), RecursiveMode::Recursive)
        .map_err(|e| format!("Failed to watch directory: {e}"))?;

    // Spawn a thread to receive events, coalesce them, and emit deduplicated changes.
    // Within each debounce window (200ms), multiple events for the same path are
    // collapsed into a single emission. This prevents rapid saves from flooding
    // the frontend with redundant directory reloads.
    std::thread::spawn(move || {
        let debounce = Duration::from_millis(200);

        loop {
            // Block until the first event arrives (or channel disconnects)
            let first = match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(event)) => event,
                Ok(Err(_)) => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };

            // Collect unique (path → kind) pairs within the debounce window.
            // Later events for the same path overwrite earlier ones.
            let mut coalesced: HashMap<String, String> = HashMap::new();

            for p in &first.paths {
                coalesced.insert(
                    p.to_string_lossy().to_string(),
                    event_kind_label(&first.kind).to_string(),
                );
            }

            // Drain additional events within the debounce window
            let deadline = Instant::now() + debounce;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match rx.recv_timeout(remaining) {
                    Ok(Ok(event)) => {
                        let kind = event_kind_label(&event.kind).to_string();
                        for p in &event.paths {
                            coalesced.insert(p.to_string_lossy().to_string(), kind.clone());
                        }
                    }
                    Ok(Err(_)) => {}
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }

            // Emit one event per unique path (deduplicated)
            for (path, kind) in &coalesced {
                let payload = FsChangePayload {
                    path: path.clone(),
                    kind: kind.clone(),
                };
                let _ = app_handle.emit("fs://change", &payload);
            }
        }
    });

    watchers.insert(path, watcher);
    Ok(())
}

#[tauri::command]
pub fn fs_unwatch_dir(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut watchers = state.watchers.lock().map_err(|e| e.to_string())?;

    // Removing the watcher drops it, which stops watching and disconnects the channel
    watchers.remove(&path);
    Ok(())
}
