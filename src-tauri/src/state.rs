use std::collections::HashMap;
use parking_lot::Mutex;

pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
    pub terminal_manager: Mutex<crate::terminal::TerminalManager>,
    pub llm_manager: Mutex<crate::llm::LlmManager>,
    pub watchers: Mutex<HashMap<String, notify::RecommendedWatcher>>,
}
