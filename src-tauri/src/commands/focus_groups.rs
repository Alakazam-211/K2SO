use tauri::{AppHandle, Emitter, State};
use crate::db::schema::{FocusGroup, Project};
use crate::project_config;
use crate::state::AppState;

#[tauri::command]
pub fn focus_groups_list(state: State<'_, AppState>) -> Result<Vec<FocusGroup>, String> {
    let conn = state.db.lock();
    FocusGroup::list(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn focus_groups_create(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    color: Option<String>,
) -> Result<FocusGroup, String> {
    let conn = state.db.lock();
    let id = uuid::Uuid::new_v4().to_string();

    let existing = FocusGroup::list(&conn).unwrap_or_default();
    let max_order = existing.iter().map(|g| g.tab_order).max().unwrap_or(-1) + 1;

    FocusGroup::create(&conn, &id, &name, color.as_deref(), max_order)
        .map_err(|e| e.to_string())?;

    let result = FocusGroup::get(&conn, &id).map_err(|e| e.to_string())?;
    let _ = app.emit("sync:focus-groups", ());
    Ok(result)
}

#[tauri::command]
pub fn focus_groups_update(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    name: Option<String>,
    color: Option<String>,
    tab_order: Option<i64>,
) -> Result<FocusGroup, String> {
    let conn = state.db.lock();
    FocusGroup::update(&conn, &id, name.as_deref(), color.as_deref(), tab_order)
        .map_err(|e| e.to_string())?;
    let result = FocusGroup::get(&conn, &id).map_err(|e| e.to_string())?;
    let _ = app.emit("sync:focus-groups", ());
    Ok(result)
}

#[tauri::command]
pub fn focus_groups_delete(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    let conn = state.db.lock();
    FocusGroup::delete(&conn, &id).map_err(|e| e.to_string())?;
    let _ = app.emit("sync:focus-groups", ());
    Ok(())
}

#[tauri::command]
pub fn focus_groups_assign_project(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
    focus_group_id: Option<String>,
) -> Result<Project, String> {
    let conn = state.db.lock();

    // Update the project's focus_group_id
    Project::update(
        &conn,
        &project_id,
        None, None, None, None, None, None,
        Some(focus_group_id.as_deref()), None, None, None, None,
    )
    .map_err(|e| e.to_string())?;

    // Write the focus group name to .k2so/config.json
    let project = Project::get(&conn, &project_id).map_err(|e| e.to_string())?;
    let group_name = match &focus_group_id {
        Some(gid) => {
            FocusGroup::get(&conn, gid)
                .ok()
                .map(|g| g.name)
        }
        None => None,
    };

    project_config::set_project_config_value(
        &project.path,
        "focusGroupName",
        group_name.as_deref(),
    )
    .ok(); // Don't fail the command if config write fails

    let result = Project::get(&conn, &project_id).map_err(|e| e.to_string())?;
    let _ = app.emit("sync:focus-groups", ());
    let _ = app.emit("sync:projects", ());
    Ok(result)
}

#[tauri::command]
pub fn focus_groups_reconcile_project(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Project, String> {
    let conn = state.db.lock();
    let project = Project::get(&conn, &project_id).map_err(|e| e.to_string())?;

    let config = project_config::get_project_config(&project.path);
    let config_group_name = config.focus_group_name;

    match config_group_name {
        None => {
            // Config says no group -- clear the DB if needed
            if project.focus_group_id.is_some() {
                Project::update(
                    &conn, &project_id, None, None, None, None, None, None,
                    Some(None), None, None, None, None,
                )
                .map_err(|e| e.to_string())?;
            }
        }
        Some(ref group_name) => {
            // Look up the group by name
            let groups = FocusGroup::list(&conn).map_err(|e| e.to_string())?;
            let existing = groups.iter().find(|g| &g.name == group_name);

            let group_id = if let Some(g) = existing {
                g.id.clone()
            } else {
                // Group doesn't exist yet -- create it
                let new_id = uuid::Uuid::new_v4().to_string();
                let max_order = groups.iter().map(|g| g.tab_order).max().unwrap_or(-1) + 1;
                FocusGroup::create(&conn, &new_id, group_name, None, max_order)
                    .map_err(|e| e.to_string())?;
                new_id
            };

            // Update if differs
            if project.focus_group_id.as_deref() != Some(&group_id) {
                Project::update(
                    &conn, &project_id, None, None, None, None, None, None,
                    Some(Some(group_id.as_str())), None, None, None, None,
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }

    let result = Project::get(&conn, &project_id).map_err(|e| e.to_string())?;
    let _ = app.emit("sync:focus-groups", ());
    let _ = app.emit("sync:projects", ());
    Ok(result)
}
