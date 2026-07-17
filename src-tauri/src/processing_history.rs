use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use tauri::AppHandle;

const HISTORY_FILE: &str = "processing-history.json";
const MAX_HISTORY_ENTRIES: usize = 500;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessingHistoryEntry {
    pub id: String,
    pub datetime: String,
    pub category: String,
    pub tool: String,
    pub source_file_name: String,
    pub created_file_name: String,
    pub created_file_path: String,
    pub status: String,
    pub original_size: Option<u64>,
    pub final_size: Option<u64>,
    pub duration_ms: Option<u64>,
    pub error_message: Option<String>,
}

fn history_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join(HISTORY_FILE))
        .map_err(|error| error.to_string())
}

fn validate_history_entry(entry: &ProcessingHistoryEntry) -> bool {
    matches!(entry.category.as_str(), "Image" | "PDF" | "Video")
        && matches!(entry.status.as_str(), "Completed" | "Failed" | "Cancelled")
        && !entry.id.is_empty()
        && !entry.datetime.is_empty()
        && !entry.tool.is_empty()
        && !entry.source_file_name.is_empty()
        && !entry.created_file_name.is_empty()
        && !entry.created_file_path.is_empty()
}

fn load_history_file(path: &PathBuf) -> Vec<ProcessingHistoryEntry> {
    let bytes = fs::read(path).ok();
    let json = bytes.as_deref().and_then(|data| std::str::from_utf8(data).ok());
    if let Some(text) = json {
        if let Ok(entries) = serde_json::from_str::<Vec<ProcessingHistoryEntry>>(text) {
            return entries.into_iter().filter(validate_history_entry).collect();
        }
    }
    Vec::new()
}

fn write_history_file(path: &PathBuf, entries: &[ProcessingHistoryEntry]) -> Result<(), String> {
    let directory = path.parent().ok_or_else(|| "Invalid history path".to_string())?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let temporary = directory.join("processing-history.json.tmp");
    let document = serde_json::to_vec_pretty(entries).map_err(|error| error.to_string())?;
    fs::write(&temporary, document).map_err(|error| error.to_string())?;
    if path.exists() {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    fs::rename(temporary, path).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn load_processing_history(app: AppHandle) -> Result<Vec<ProcessingHistoryEntry>, String> {
    let path = history_path(&app)?;
    if !path.is_file() {
        return Ok(Vec::new());
    }
    Ok(load_history_file(&path))
}

#[tauri::command]
pub fn save_processing_history(
    app: AppHandle,
    entries: Vec<ProcessingHistoryEntry>,
) -> Result<Vec<ProcessingHistoryEntry>, String> {
    let path = history_path(&app)?;
    let mut trimmed = entries.into_iter().filter(validate_history_entry).collect::<Vec<_>>();
    if trimmed.len() > MAX_HISTORY_ENTRIES {
        trimmed = trimmed.split_off(trimmed.len() - MAX_HISTORY_ENTRIES);
    }
    write_history_file(&path, &trimmed)?;
    Ok(trimmed)
}

#[tauri::command]
pub fn append_processing_history_entry(
    app: AppHandle,
    entry: ProcessingHistoryEntry,
) -> Result<Vec<ProcessingHistoryEntry>, String> {
    if !validate_history_entry(&entry) {
        return Err("Invalid history entry".into());
    }
    let path = history_path(&app)?;
    let mut entries = if path.is_file() { load_history_file(&path) } else { Vec::new() };
    entries.push(entry);
    if entries.len() > MAX_HISTORY_ENTRIES {
        entries = entries.split_off(entries.len() - MAX_HISTORY_ENTRIES);
    }
    write_history_file(&path, &entries)?;
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    use tauri::AppHandle;

    fn fake_handle(dir: PathBuf) -> AppHandle {
        struct FakeHandle(PathBuf);
        impl AppHandle {
            fn path(&self) -> tauri::api::path::AppPaths { unimplemented!() }
        }
        unimplemented!()
    }
}
