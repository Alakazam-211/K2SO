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
                    // Use IF NOT EXISTS for CREATE TABLE, ignore errors for ALTER TABLE
                    // (handles migrating from Electron's Drizzle-managed DB)
                    match conn.execute_batch(trimmed) {
                        Ok(_) => {},
                        Err(e) => {
                            let msg = e.to_string();
                            // Ignore "already exists" and "duplicate column" errors
                            if msg.contains("already exists") || msg.contains("duplicate column") {
                                eprintln!("[db] Migration {}: skipping ({})", name, msg);
                            } else {
                                return Err(e);
                            }
                        }
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
        // Cloud CLI agents
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456001", "Claude", "claude --dangerously-skip-permissions", "\u{1F916}", 0),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456002", "Codex", "codex -c model_reasoning_effort=\"high\" --dangerously-bypass-approvals-and-sandbox", "\u{1F98E}", 1),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456003", "Gemini", "gemini --yolo", "\u{1F48E}", 2),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456004", "Copilot", "copilot --allow-all", "\u{1F6F8}", 3),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456005", "Aider", "aider", "\u{1F6E0}", 4),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456006", "Cursor Agent", "cursor-agent", "\u{26A1}", 5),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456007", "OpenCode", "opencode", "\u{1F4DF}", 6),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456008", "Code Puppy", "codepuppy", "\u{1F436}", 7),
        // Local/on-device LLM tools
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
