//! Tauri-backed `TerminalEventSink` implementation.
//!
//! Wraps a `tauri::AppHandle` and forwards core's terminal events to the
//! existing `terminal:title:<id>` / `terminal:bell:<id>` / `terminal:exit:<id>`
//! / `terminal:grid:<id>` event channels the React frontend already listens
//! to. Lets the terminal module live in k2so-core without taking a Tauri
//! dependency while keeping the renderer contract identical.

use std::sync::Arc;

use k2so_core::terminal::event_sink::TerminalEventSink;
use k2so_core::terminal::grid_types::GridUpdate;
use tauri::{AppHandle, Emitter};

/// Event sink that forwards to a Tauri `AppHandle`. Share it as an
/// `Arc<dyn TerminalEventSink>` — `TerminalManager::create` takes that
/// trait object.
pub struct TauriTerminalEventSink {
    app_handle: AppHandle,
}

impl TauriTerminalEventSink {
    pub fn new(app_handle: AppHandle) -> Arc<dyn TerminalEventSink> {
        Arc::new(Self { app_handle })
    }
}

impl TerminalEventSink for TauriTerminalEventSink {
    fn on_title(&self, terminal_id: &str, title: &str) {
        let _ = self
            .app_handle
            .emit(&format!("terminal:title:{}", terminal_id), title);
    }

    fn on_bell(&self, terminal_id: &str) {
        let _ = self
            .app_handle
            .emit(&format!("terminal:bell:{}", terminal_id), ());
    }

    fn on_exit(&self, terminal_id: &str, exit_code: i32) {
        let _ = self.app_handle.emit(
            &format!("terminal:exit:{}", terminal_id),
            serde_json::json!({ "exitCode": exit_code }),
        );
    }

    fn on_grid_update(&self, terminal_id: &str, update: &GridUpdate) {
        let _ = self
            .app_handle
            .emit(&format!("terminal:grid:{}", terminal_id), update);
    }
}
