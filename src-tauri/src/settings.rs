use serde::{Deserialize, Serialize};
use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use tauri::{AppHandle, Manager};

const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub language: String,
    pub appearance: String,
    pub default_output_folder: Option<String>,
    pub keep_original_files: bool,
    pub never_overwrite_existing_files: bool,
    pub open_output_folder_after_processing: bool,
    pub enable_processing_history: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            language: "en".into(),
            appearance: "system".into(),
            default_output_folder: None,
            keep_original_files: true,
            never_overwrite_existing_files: true,
            open_output_folder_after_processing: false,
            enable_processing_history: true,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub pdfium: bool,
    pub ffmpeg: bool,
    pub ffprobe: bool,
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join(SETTINGS_FILE))
        .map_err(|error| error.to_string())
}

fn validate(mut settings: AppSettings) -> Result<AppSettings, String> {
    if !matches!(settings.language.as_str(), "en" | "fr" | "ar") {
        return Err("Unsupported language".into());
    }
    if !matches!(settings.appearance.as_str(), "system" | "light" | "dark") {
        return Err("Unsupported appearance".into());
    }
    if let Some(folder) = settings.default_output_folder.as_deref() {
        let path = Path::new(folder);
        if !path.is_absolute() || !path.is_dir() {
            return Err("The default output folder is unavailable".into());
        }
    }
    // These two protections are deliberately not user-configurable.
    settings.keep_original_files = true;
    settings.never_overwrite_existing_files = true;
    Ok(settings)
}

#[tauri::command]
pub fn load_settings(app: AppHandle) -> Result<AppSettings, String> {
    let path = settings_path(&app)?;
    if !path.is_file() {
        return Ok(AppSettings::default());
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(bytes.as_slice())
        .map_err(|error| error.to_string())
        .and_then(validate)
        .or_else(|_| Ok(AppSettings::default()))
}

#[tauri::command]
pub fn save_settings(app: AppHandle, settings: AppSettings) -> Result<AppSettings, String> {
    let settings = validate(settings)?;
    let path = settings_path(&app)?;
    let directory = path.parent().ok_or("Invalid settings directory")?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let temporary = directory.join("settings.json.tmp");
    let document = serde_json::to_vec_pretty(&settings).map_err(|error| error.to_string())?;
    fs::write(&temporary, document).map_err(|error| error.to_string())?;
    if path.exists() {
        fs::remove_file(&path).map_err(|error| error.to_string())?;
    }
    fs::rename(temporary, path).map_err(|error| error.to_string())?;
    Ok(settings)
}

#[tauri::command]
pub fn get_runtime_status(app: AppHandle) -> RuntimeStatus {
    RuntimeStatus {
        pdfium: pdfium_is_available(&app),
        ffmpeg: video_tool_is_available(&app, "ffmpeg"),
        ffprobe: video_tool_is_available(&app, "ffprobe"),
    }
}

fn pdfium_is_available(app: &AppHandle) -> bool {
    let mut candidates = Vec::new();
    if let Ok(path) = app.path().resolve("pdfium/pdfium.dll", tauri::path::BaseDirectory::Resource) {
        candidates.push(path);
    }
    if let Ok(executable) = env::current_exe() {
        if let Some(directory) = executable.parent() {
            candidates.push(directory.join("pdfium/pdfium.dll"));
            candidates.push(directory.join("pdfium.dll"));
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/pdfium/pdfium.dll"));
    candidates.into_iter().any(|path| path.is_file())
}

fn video_tool_is_available(app: &AppHandle, name: &str) -> bool {
    let suffixed = format!("{name}-x86_64-pc-windows-msvc.exe");
    let plain = format!("{name}.exe");
    let mut candidates = Vec::new();
    if cfg!(debug_assertions) {
        candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries").join(&suffixed));
    }
    if let Ok(executable) = env::current_exe() {
        if let Some(directory) = executable.parent() {
            candidates.push(directory.join(&plain));
            candidates.push(directory.join(&suffixed));
        }
    }
    if let Ok(resources) = app.path().resource_dir() {
        candidates.push(resources.join(&plain));
        candidates.push(resources.join("binaries").join(&plain));
    }
    let Some(binary) = candidates.into_iter().find(|path| path.is_file()) else { return false; };
    let library_dir = if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/ffmpeg")
    } else {
        app.path().resource_dir().map(|path| path.join("ffmpeg")).unwrap_or_else(|_| PathBuf::from("ffmpeg"))
    };
    let existing = env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![library_dir];
    paths.extend(env::split_paths(&existing));
    let runtime_path: OsString = env::join_paths(paths).unwrap_or(existing);
    Command::new(binary).arg("-version").env("PATH", runtime_path).output()
        .is_ok_and(|output| output.status.success())
}

#[tauri::command]
pub fn get_support_document_path(app: AppHandle, document: String) -> Result<String, String> {
    let file_name = match document.as_str() {
        "licenses" => "THIRD_PARTY_LICENSES.txt",
        "documentation" => "README.md",
        _ => return Err("Unknown support document".into()),
    };
    let mut candidates = Vec::new();
    if let Ok(path) = app
        .path()
        .resolve(file_name, tauri::path::BaseDirectory::Resource)
    {
        candidates.push(path);
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(file_name),
    );
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
        .ok_or_else(|| "Support document unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_keep_both_file_safety_guards_enabled() {
        let defaults = AppSettings::default();
        assert!(defaults.keep_original_files);
        assert!(defaults.never_overwrite_existing_files);
        assert!(defaults.enable_processing_history);
        assert_eq!(defaults.default_output_folder, None);
    }

    #[test]
    fn validation_relocks_file_safety_guards() {
        let mut settings = AppSettings::default();
        settings.keep_original_files = false;
        settings.never_overwrite_existing_files = false;
        let validated = validate(settings).unwrap();
        assert!(validated.keep_original_files);
        assert!(validated.never_overwrite_existing_files);
    }
}
