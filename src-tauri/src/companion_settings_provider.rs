//! Tauri-side implementation of
//! `k2so_core::companion::settings_bridge::CompanionSettingsProvider`.
//!
//! Registers at startup (in `lib.rs::setup()`) so the companion module
//! — which now lives in k2so-core — can read the user's companion
//! credentials through a narrow trait interface instead of reaching
//! back into `crate::commands::settings::*`. Also exposes the inverse:
//! `clear_password_hash_after_migration` still writes to the same
//! settings.json the Tauri app owns.

use k2so_core::companion::settings_bridge::{
    CompanionSettingsProvider, CompanionSettingsSnapshot,
};

pub struct TauriCompanionSettingsProvider;

impl CompanionSettingsProvider for TauriCompanionSettingsProvider {
    fn read(&self) -> CompanionSettingsSnapshot {
        let settings = crate::commands::settings::read_settings();
        let c = &settings.companion;
        CompanionSettingsSnapshot {
            username: c.username.clone(),
            password_hash: c.password_hash.clone(),
            password_set: c.password_set,
            ngrok_auth_token: c.ngrok_auth_token.clone(),
            ngrok_domain: c.ngrok_domain.clone(),
            cors_origins: c.cors_origins.clone(),
            allow_remote_spawn: c.allow_remote_spawn,
        }
    }

    fn clear_password_hash_after_migration(&self) {
        crate::commands::settings::clear_companion_password_hash_after_migration();
    }
}
