use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use tauri::{AppHandle, Manager};

const TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";

pub fn log_tool_versions(app: &AppHandle) {
    let library_dir = runtime_library_dir(app);

    for (display_name, binary_name) in [("FFmpeg", "ffmpeg"), ("FFprobe", "ffprobe")] {
        match find_sidecar(app, binary_name)
            .and_then(|path| version_line(&path, &library_dir).map(|line| (path, line)))
        {
            Ok((path, line)) => {
                eprintln!("[video-tools] {display_name}: {line} ({})", path.display());
            }
            Err(error) => {
                eprintln!("[video-tools] {display_name} unavailable: {error}");
            }
        }
    }
}

fn runtime_library_dir(app: &AppHandle) -> PathBuf {
    if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/ffmpeg")
    } else {
        app.path()
            .resource_dir()
            .map(|path| path.join("ffmpeg"))
            .unwrap_or_else(|_| PathBuf::from("ffmpeg"))
    }
}

fn find_sidecar(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    let suffixed = format!("{name}-{TARGET_TRIPLE}.exe");
    let plain = format!("{name}.exe");
    let mut candidates = Vec::new();

    if cfg!(debug_assertions) {
        candidates.push(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("binaries")
                .join(&suffixed),
        );
    }

    if let Ok(current_exe) = env::current_exe() {
        if let Some(directory) = current_exe.parent() {
            candidates.push(directory.join(&plain));
            candidates.push(directory.join(&suffixed));
        }
    }

    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join(&plain));
        candidates.push(resource_dir.join("binaries").join(&plain));
    }

    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| format!("bundled sidecar {plain} was not found"))
}

fn version_line(binary: &Path, library_dir: &Path) -> Result<String, String> {
    let output = Command::new(binary)
        .arg("-version")
        .env("PATH", path_with_runtime_libraries(library_dir))
        .output()
        .map_err(|error| format!("could not start {}: {error}", binary.display()))?;

    if !output.status.success() {
        return Err(format!(
            "{} -version exited with {}",
            binary.display(),
            output.status
        ));
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| format!("{} returned no version", binary.display()))
}

fn path_with_runtime_libraries(library_dir: &Path) -> OsString {
    let existing = env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![library_dir.to_path_buf()];
    paths.extend(env::split_paths(&existing));
    env::join_paths(paths).unwrap_or(existing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_video_tools_are_lgpl_and_have_the_expected_version() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let library_dir = manifest_dir.join("resources/ffmpeg");

        for name in ["ffmpeg", "ffprobe"] {
            let binary = manifest_dir
                .join("binaries")
                .join(format!("{name}-{TARGET_TRIPLE}.exe"));
            let output = Command::new(&binary)
                .arg("-version")
                .env("PATH", path_with_runtime_libraries(&library_dir))
                .output()
                .expect("bundled video sidecar must start");
            let stdout = String::from_utf8_lossy(&output.stdout);

            assert!(output.status.success());
            assert!(stdout.contains("n7.1.5-1-g7d0e842004-20260630"));
            assert!(stdout.contains("--enable-shared"));
            assert!(stdout.contains("--disable-static"));
            assert!(!stdout.contains("--enable-gpl"));
            assert!(!stdout.contains("--enable-nonfree"));
        }
    }
}
