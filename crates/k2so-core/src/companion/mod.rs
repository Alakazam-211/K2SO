//! Companion runtime: WebSocket server + ngrok tunnel + mobile-facing
//! HTTP routes.
//!
//! This module is being migrated from `src-tauri/src/companion/` in
//! pieces. Today k2so-core hosts the **pure** sub-modules (auth,
//! keychain, HTTP proxy, shared types, WebSocket broadcast helpers)
//! which have zero Tauri dependencies. The Tauri-coupled lifecycle
//! orchestrator (tunnel start/stop, event forwarding, terminal
//! polling) still lives in src-tauri but re-imports everything here
//! via `use k2so_core::companion::*`.
//!
//! The follow-up commit introduces a `CompanionEventSink` trait and
//! relocates the orchestrator too.

pub mod app_event_source;
pub mod auth;
pub mod event_sink;
pub mod keychain;
pub mod proxy;
pub mod settings_bridge;
pub mod terminal_bridge;
pub mod types;
pub mod websocket;
