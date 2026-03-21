use crate::project_config;

#[tauri::command]
pub fn project_config_get(path: String) -> Result<project_config::ProjectConfig, String> {
    Ok(project_config::get_project_config(&path))
}

#[tauri::command]
pub fn project_config_has_run_command(path: String) -> Result<bool, String> {
    Ok(project_config::has_run_command(&path))
}

#[derive(serde::Serialize)]
pub struct RunCommandResult {
    pub command: String,
}

#[tauri::command]
pub fn project_config_run_command(path: String) -> Result<RunCommandResult, String> {
    let config = project_config::get_project_config(&path);
    match config.run_command {
        Some(cmd) if !cmd.is_empty() => Ok(RunCommandResult { command: cmd }),
        _ => Err("No run command configured for this project".to_string()),
    }
}
