use crate::state::AppState;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;
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

    // Spawn a thread to receive events and debounce them
    std::thread::spawn(move || {
        let mut last_emit = std::time::Instant::now();
        let debounce = Duration::from_millis(200);

        loop {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(event)) => {
                    let now = std::time::Instant::now();
                    if now.duration_since(last_emit) < debounce {
                        // Drain any queued events within the debounce window
                        while rx.recv_timeout(debounce).is_ok() {}
                    }
                    last_emit = now;

                    let kind_label = event_kind_label(&event.kind);
                    for p in &event.paths {
                        let payload = FsChangePayload {
                            path: p.to_string_lossy().to_string(),
                            kind: kind_label.to_string(),
                        };
                        let _ = app_handle.emit("fs://change", &payload);
                    }
                }
                Ok(Err(_)) => {
                    // Watcher error, continue
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // No events, keep waiting
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // Watcher was dropped, exit thread
                    break;
                }
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
