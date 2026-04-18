use crate::db::schema::TimeEntry;
use crate::state::AppState;
use tauri::{AppHandle, Emitter, State};

#[tauri::command]
pub fn timer_entries_list(
    state: State<'_, AppState>,
    start: Option<i64>,
    end: Option<i64>,
    project_id: Option<String>,
) -> Result<Vec<TimeEntry>, String> {
    let conn = state.db.lock();
    TimeEntry::list(&conn, start, end, project_id.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn timer_entry_create(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    project_id: Option<String>,
    start_time: i64,
    end_time: i64,
    duration_seconds: i64,
    memo: Option<String>,
) -> Result<(), String> {
    let conn = state.db.lock();
    TimeEntry::create(
        &conn,
        &id,
        project_id.as_deref(),
        start_time,
        end_time,
        duration_seconds,
        memo.as_deref(),
    )
    .map_err(|e| e.to_string())?;
    drop(conn);
    let _ = app.emit("sync:timer-entries", ());
    Ok(())
}

#[tauri::command]
pub fn timer_entry_delete(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let conn = state.db.lock();
    TimeEntry::delete(&conn, id.as_str()).map_err(|e| e.to_string())?;
    drop(conn);
    let _ = app.emit("sync:timer-entries", ());
    Ok(())
}

#[tauri::command]
pub fn timer_entries_export(
    state: State<'_, AppState>,
    format: String,
    start: Option<i64>,
    end: Option<i64>,
    project_id: Option<String>,
) -> Result<String, String> {
    let conn = state.db.lock();
    let entries =
        TimeEntry::list(&conn, start, end, project_id.as_deref()).map_err(|e| e.to_string())?;

    match format.as_str() {
        "csv" => {
            let mut csv = String::from("id,project_id,start_time,end_time,duration_seconds,memo,created_at\n");
            for e in &entries {
                csv.push_str(&format!(
                    "{},{},{},{},{},{},{}\n",
                    e.id,
                    e.project_id.as_deref().unwrap_or(""),
                    e.start_time,
                    e.end_time,
                    e.duration_seconds,
                    csv_escape(e.memo.as_deref().unwrap_or("")),
                    e.created_at,
                ));
            }
            Ok(csv)
        }
        _ => {
            serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())
        }
    }
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
