//! Tauri-side glue for k2so-core's companion bridges.
//!
//! Registers implementations of `TerminalProvider`, `CompanionEventSink`,
//! and `AppEventSource` that wrap Tauri's `AppHandle` + `AppState` so
//! the companion module — which lives in k2so-core and therefore can't
//! touch the Tauri API directly — can still emit UI events, subscribe
//! to inbound app events, and read terminal snapshots.
//!
//! Call [`register`] once during `setup()` before any companion code
//! runs.

use std::sync::Arc;

use k2so_core::companion::{
    app_event_source::{AppEventHandler, AppEventSource},
    event_sink::CompanionEventSink,
    terminal_bridge::TerminalProvider,
};
use k2so_core::terminal::grid_types::GridUpdate;
use parking_lot::Mutex as PLMutex;
use tauri::{AppHandle, Emitter, Listener, Manager};

/// Reads terminal grids + scrollback through the AppState's
/// `terminal_manager`. Held by an Arc so the caller can construct
/// once and pass to `set_provider`.
struct TauriTerminalProvider {
    app_handle: AppHandle,
}

impl TerminalProvider for TauriTerminalProvider {
    fn get_grid(&self, terminal_id: &str) -> Result<GridUpdate, String> {
        let state = self
            .app_handle
            .try_state::<crate::state::AppState>()
            .ok_or_else(|| "AppState unavailable".to_string())?;
        let manager = state.terminal_manager.lock();
        manager.get_grid(terminal_id)
    }

    fn read_lines_with_scrollback(
        &self,
        terminal_id: &str,
        limit: usize,
        include_scrollback: bool,
    ) -> Result<Vec<String>, String> {
        let state = self
            .app_handle
            .try_state::<crate::state::AppState>()
            .ok_or_else(|| "AppState unavailable".to_string())?;
        let manager = state.terminal_manager.lock();
        manager.read_lines_with_scrollback(terminal_id, limit, include_scrollback)
    }
}

/// Routes core's `emit(event, payload)` back onto Tauri's event bus so
/// the existing React listeners keep receiving `companion:*` events.
struct TauriCompanionEventSink {
    app_handle: AppHandle,
}

impl CompanionEventSink for TauriCompanionEventSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = self.app_handle.emit(event, payload);
    }
}

/// When companion subscribes to a set of app events, we register Tauri
/// listeners that forward payloads to the supplied handler. Handlers
/// are kept alive for the lifetime of the app (stored in a static vec).
struct TauriAppEventSource {
    app_handle: AppHandle,
}

/// Holds strong refs to every handler companion has subscribed so they
/// survive closure drops. Keyed by nothing — drop semantics match the
/// app process lifetime.
static HANDLER_KEEPALIVE: PLMutex<Vec<Arc<AppEventHandler>>> = PLMutex::new(Vec::new());

impl AppEventSource for TauriAppEventSource {
    fn subscribe(&self, events: &[&'static str], handler: AppEventHandler) {
        let handler_arc: Arc<AppEventHandler> = Arc::new(handler);
        HANDLER_KEEPALIVE.lock().push(handler_arc.clone());
        for event_name in events.iter().copied() {
            let handler_for_listener = handler_arc.clone();
            self.app_handle.listen(event_name.to_string(), move |event| {
                let payload = event.payload();
                (handler_for_listener)(event_name, payload);
            });
        }
    }
}

/// Register all three Tauri-side bridges. Must run before the companion
/// module emits / subscribes / reads terminals for the first time.
pub fn register(app_handle: AppHandle) {
    k2so_core::companion::terminal_bridge::set_provider(Box::new(TauriTerminalProvider {
        app_handle: app_handle.clone(),
    }));
    k2so_core::companion::event_sink::set_sink(Box::new(TauriCompanionEventSink {
        app_handle: app_handle.clone(),
    }));
    k2so_core::companion::app_event_source::set_source(Box::new(TauriAppEventSource {
        app_handle,
    }));
}
