pub mod schema;

use rusqlite::{params, Connection, Result};

/// Open (or create) the K2SO database at ~/.k2so/k2so.db,
/// run all migrations, and seed default data.
pub fn init_database() -> Result<Connection> {
    let db_dir = dirs::home_dir()
        .ok_or_else(|| rusqlite::Error::InvalidParameterName("Could not determine home directory".to_string()))?
        .join(".k2so");
    std::fs::create_dir_all(&db_dir)
        .map_err(|e| rusqlite::Error::InvalidParameterName(format!("Could not create ~/.k2so directory: {}", e)))?;

    let db_path = db_dir.join("k2so.db");
    let conn = Connection::open(db_path)?;

    // Enable WAL mode for better concurrent read performance
    conn.execute_batch("PRAGMA journal_mode = WAL;")?;
    // Enable foreign key enforcement
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;

    run_migrations(&conn)?;
    seed_agent_presets(&conn)?;

    Ok(conn)
}

/// Simple migration runner using a _migrations table to track applied migrations.
fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            applied_at INTEGER NOT NULL DEFAULT (unixepoch())
        );",
    )?;

    let migrations: &[(&str, &str)] = &[
        ("0000_lethal_scalphunter", include_str!("../../drizzle_sql/0000_lethal_scalphunter.sql")),
        ("0001_nostalgic_lenny_balinger", include_str!("../../drizzle_sql/0001_nostalgic_lenny_balinger.sql")),
        ("0002_fearless_photon", include_str!("../../drizzle_sql/0002_fearless_photon.sql")),
        ("0003_fancy_thunderball", include_str!("../../drizzle_sql/0003_fancy_thunderball.sql")),
        ("0004_pinned_workspaces", include_str!("../../drizzle_sql/0004_pinned_workspaces.sql")),
        ("0005_window_state", include_str!("../../drizzle_sql/0005_window_state.sql")),
        ("0006_time_entries", include_str!("../../drizzle_sql/0006_time_entries.sql")),
        ("0007_chat_session_names", include_str!("../../drizzle_sql/0007_chat_session_names.sql")),
        ("0008_chat_pinned", include_str!("../../drizzle_sql/0008_chat_pinned.sql")),
        ("0009_workspace_sessions", include_str!("../../drizzle_sql/0009_workspace_sessions.sql")),
    ];

    for (name, sql) in migrations {
        let already_applied: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM _migrations WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;

        if !already_applied {
            // Split on the drizzle statement-breakpoint marker and execute each statement
            for statement in sql.split("--> statement-breakpoint") {
                let trimmed = statement.trim();
                if !trimmed.is_empty() {
                    // Retry with backoff for database lock contention
                    let mut last_err = None;
                    for attempt in 0..5u32 {
                        match conn.execute_batch(trimmed) {
                            Ok(_) => { last_err = None; break; },
                            Err(e) => {
                                let msg = e.to_string();
                                // Ignore "already exists" and "duplicate column" errors
                                if msg.contains("already exists") || msg.contains("duplicate column") {
                                    log_debug!("[db] Migration {}: skipping ({})", name, msg);
                                    last_err = None;
                                    break;
                                }
                                // Retry on lock contention
                                if (msg.contains("database is locked") || msg.contains("schema is locked"))
                                    && attempt < 4
                                {
                                    log_debug!("[db] Migration {}: locked, retrying ({}/5)", name, attempt + 1);
                                    std::thread::sleep(std::time::Duration::from_millis(50 * (attempt as u64 + 1)));
                                    last_err = Some(e);
                                    continue;
                                }
                                return Err(e);
                            }
                        }
                    }
                    if let Some(e) = last_err {
                        return Err(e);
                    }
                }
            }
            conn.execute(
                "INSERT INTO _migrations (name) VALUES (?1)",
                params![name],
            )?;
        }
    }

    Ok(())
}

/// Seed built-in agent presets. Uses INSERT OR IGNORE so new presets
/// are added on upgrade without duplicating existing ones.
fn seed_agent_presets(conn: &Connection) -> Result<()> {
    let presets: &[(&str, &str, &str, &str, i64)] = &[
        // Cloud CLI agents (no emoji — use custom AgentIcon SVGs)
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456001", "Claude", "claude --dangerously-skip-permissions", "", 0),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456002", "Codex", "codex -c model_reasoning_effort=\"high\" --dangerously-bypass-approvals-and-sandbox", "", 1),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456003", "Gemini", "gemini --yolo", "", 2),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456004", "Copilot", "copilot --allow-all", "", 3),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456005", "Aider", "aider", "", 4),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456006", "Cursor Agent", "cursor-agent", "", 5),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456007", "OpenCode", "opencode", "", 6),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456008", "Code Puppy", "codepuppy", "", 7),
        // Local/on-device LLM tools (keep emoji — no custom icon)
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456009", "Ollama", "ollama run llama3.2", "\u{1F999}", 8),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456010", "Interpreter", "interpreter", "\u{1F310}", 9),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456011", "Goose", "goose", "\u{1FABF}", 10),
    ];

    for (id, label, command, icon, sort_order) in presets {
        conn.execute(
            "INSERT OR IGNORE INTO agent_presets (id, label, command, icon, enabled, sort_order, is_built_in) \
             VALUES (?1, ?2, ?3, ?4, 1, ?5, 1)",
            params![id, label, command, icon, sort_order],
        )?;
    }

    Ok(())
}
