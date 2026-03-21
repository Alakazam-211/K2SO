import { sqliteTable, text, integer, real } from 'drizzle-orm/sqlite-core'
import { sql, type InferSelectModel, type InferInsertModel } from 'drizzle-orm'

// ── Focus Groups ─────────────────────────────────────────────────────────────

export const focusGroups = sqliteTable('focus_groups', {
  id: text('id').primaryKey(),
  name: text('name').notNull(),
  color: text('color'),
  tabOrder: integer('tab_order').notNull().default(0),
  createdAt: integer('created_at')
    .notNull()
    .default(sql`(unixepoch())`)
})

export type FocusGroup = InferSelectModel<typeof focusGroups>
export type NewFocusGroup = InferInsertModel<typeof focusGroups>

// ── Projects ──────────────────────────────────────────────────────────────────

export const projects = sqliteTable('projects', {
  id: text('id').primaryKey(),
  name: text('name').notNull(),
  path: text('path').notNull().unique(),
  color: text('color').notNull().default('#3b82f6'),
  tabOrder: integer('tab_order').notNull().default(0),
  lastOpenedAt: integer('last_opened_at'),
  worktreeMode: integer('worktree_mode').notNull().default(0),
  iconUrl: text('icon_url'),
  focusGroupId: text('focus_group_id').references(() => focusGroups.id, { onDelete: 'set null' }),
  createdAt: integer('created_at')
    .notNull()
    .default(sql`(unixepoch())`)
})

export type Project = InferSelectModel<typeof projects>
export type NewProject = InferInsertModel<typeof projects>

// ── Workspace Sections (Focus Groups) ────────────────────────────────────────

export const workspaceSections = sqliteTable('workspace_sections', {
  id: text('id').primaryKey(),
  projectId: text('project_id')
    .notNull()
    .references(() => projects.id, { onDelete: 'cascade' }),
  name: text('name').notNull(),
  color: text('color'),
  isCollapsed: integer('is_collapsed').notNull().default(0),
  tabOrder: integer('tab_order').notNull().default(0),
  createdAt: integer('created_at')
    .notNull()
    .default(sql`(unixepoch())`)
})

export type WorkspaceSection = InferSelectModel<typeof workspaceSections>
export type NewWorkspaceSection = InferInsertModel<typeof workspaceSections>

// ── Workspaces ────────────────────────────────────────────────────────────────

export const workspaces = sqliteTable('workspaces', {
  id: text('id').primaryKey(),
  projectId: text('project_id')
    .notNull()
    .references(() => projects.id, { onDelete: 'cascade' }),
  sectionId: text('section_id').references(() => workspaceSections.id, { onDelete: 'set null' }),
  type: text('type').notNull().default('branch'),
  branch: text('branch'),
  name: text('name').notNull(),
  tabOrder: integer('tab_order').notNull().default(0),
  worktreePath: text('worktree_path'),
  createdAt: integer('created_at')
    .notNull()
    .default(sql`(unixepoch())`)
})

export type Workspace = InferSelectModel<typeof workspaces>
export type NewWorkspace = InferInsertModel<typeof workspaces>

// ── Agent Presets ─────────────────────────────────────────────────────────────

export const agentPresets = sqliteTable('agent_presets', {
  id: text('id').primaryKey(),
  label: text('label').notNull(),
  command: text('command').notNull(),
  icon: text('icon'),
  enabled: integer('enabled').notNull().default(1),
  sortOrder: integer('sort_order').notNull().default(0),
  isBuiltIn: integer('is_built_in').notNull().default(0),
  createdAt: integer('created_at')
    .notNull()
    .default(sql`(unixepoch())`)
})

export type AgentPreset = InferSelectModel<typeof agentPresets>
export type NewAgentPreset = InferInsertModel<typeof agentPresets>

// ── Terminal Tabs ─────────────────────────────────────────────────────────────

export const terminalTabs = sqliteTable('terminal_tabs', {
  id: text('id').primaryKey(),
  workspaceId: text('workspace_id')
    .notNull()
    .references(() => workspaces.id, { onDelete: 'cascade' }),
  title: text('title').notNull().default('Terminal'),
  tabOrder: integer('tab_order').notNull().default(0),
  createdAt: integer('created_at')
    .notNull()
    .default(sql`(unixepoch())`)
})

export type TerminalTab = InferSelectModel<typeof terminalTabs>
export type NewTerminalTab = InferInsertModel<typeof terminalTabs>

// ── Terminal Panes ────────────────────────────────────────────────────────────

export const terminalPanes = sqliteTable('terminal_panes', {
  id: text('id').primaryKey(),
  tabId: text('tab_id')
    .notNull()
    .references(() => terminalTabs.id, { onDelete: 'cascade' }),
  splitDirection: text('split_direction'),
  splitRatio: real('split_ratio').default(0.5),
  paneOrder: integer('pane_order').notNull().default(0),
  createdAt: integer('created_at')
    .notNull()
    .default(sql`(unixepoch())`)
})

export type TerminalPane = InferSelectModel<typeof terminalPanes>
export type NewTerminalPane = InferInsertModel<typeof terminalPanes>
