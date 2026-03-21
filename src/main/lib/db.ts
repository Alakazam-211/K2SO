import { join } from 'path'
import { mkdirSync } from 'fs'
import { homedir } from 'os'
import Database, { type Database as DatabaseType } from 'better-sqlite3'
import { drizzle } from 'drizzle-orm/better-sqlite3'
import { migrate } from 'drizzle-orm/better-sqlite3/migrator'
import { count } from 'drizzle-orm'
import { dialog } from 'electron'
import * as schema from './db/schema'
import { builtInAgentPresets } from '../../shared/agent-catalog'

// ── Database path ─────────────────────────────────────────────────────────────

const DB_DIR = join(homedir(), '.k2so')
const DB_PATH = join(DB_DIR, 'k2so.db')

// Ensure the directory exists
mkdirSync(DB_DIR, { recursive: true })

// ── Open SQLite with WAL mode ─────────────────────────────────────────────────

export const sqlite: DatabaseType = new Database(DB_PATH)
sqlite.pragma('journal_mode = WAL')
sqlite.pragma('foreign_keys = ON')

// ── Drizzle instance ──────────────────────────────────────────────────────────

export const db = drizzle(sqlite, { schema })

// ── Run migrations ────────────────────────────────────────────────────────────

try {
  migrate(db, { migrationsFolder: join(__dirname, '../../drizzle') })
} catch (err) {
  console.error('[db] Migration failed:', err)
  try {
    dialog.showErrorBox(
      'Database Migration Failed',
      'Database migration failed. The app may not work correctly.'
    )
  } catch {
    // dialog may not be available if app is not ready yet
  }
}

// ── Seed default agent presets ────────────────────────────────────────────────

function seedAgentPresets(): void {
  const [result] = db.select({ total: count() }).from(schema.agentPresets).all()
  if (result.total === 0) {
    for (const preset of builtInAgentPresets) {
      db.insert(schema.agentPresets)
        .values({
          id: preset.id,
          label: preset.label,
          command: preset.command,
          icon: preset.icon,
          enabled: preset.enabled,
          isBuiltIn: preset.isBuiltIn,
          sortOrder: preset.sortOrder
        })
        .run()
    }
  }
}

try {
  seedAgentPresets()
} catch (err) {
  console.error('[db] Failed to seed agent presets:', err)
}
