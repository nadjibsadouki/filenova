use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use serde::Serialize;
use tauri::{AppHandle, Manager};

pub(crate) const TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";
const ACCEPTED_VIDEO_EXTENSIONS: [&str; 6] = ["mp4", "mov", "mkv", "avi", "webm", "m4v"];

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoInfo {
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) format: String,
    pub(crate) duration_seconds: f64,
    pub(crate) width: u64,
    pub(crate) height: u64,
    pub(crate) size: u64,
    pub(crate) video_codec: String,
    pub(crate) audio_codec: Option<String>,
    pub(crate) thumbnail_data: Option<Vec<u8>>,
}

#[derive(Debug, Serialize)]
pub struct VideoInspectError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl VideoInspectError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

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

pub(crate) fn runtime_library_dir(app: &AppHandle) -> PathBuf {
    if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/ffmpeg")
    } else {
        app.path()
            .resource_dir()
            .map(|path| path.join("ffmpeg"))
            .unwrap_or_else(|_| PathBuf::from("ffmpeg"))
    }
}

pub(crate) fn find_sidecar(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
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

pub(crate) fn path_with_runtime_libraries(library_dir: &Path) -> OsString {
    let existing = env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![library_dir.to_path_buf()];
    paths.extend(env::split_paths(&existing));
    env::join_paths(paths).unwrap_or(existing)
}

#[tauri::command]
pub async fn inspect_video(app: AppHandle, path: String) -> Result<VideoInfo, VideoInspectError> {
    let ffprobe = find_sidecar(&app, "ffprobe")
        .map_err(|error| VideoInspectError::new("ffprobe_unavailable", error))?;
    let ffmpeg = find_sidecar(&app, "ffmpeg")
        .map_err(|error| VideoInspectError::new("ffmpeg_unavailable", error))?;
    let library_dir = runtime_library_dir(&app);

    tauri::async_runtime::spawn_blocking(move || {
        inspect_video_with_tools(Path::new(&path), &ffprobe, &ffmpeg, &library_dir)
    })
    .await
    .map_err(|error| VideoInspectError::new("inspection_failed", error.to_string()))?
}

pub(crate) fn inspect_video_with_tools(
    path: &Path,
    ffprobe: &Path,
    ffmpeg: &Path,
    library_dir: &Path,
) -> Result<VideoInfo, VideoInspectError> {
    if !path.is_absolute() || !path.is_file() {
        return Err(VideoInspectError::new(
            "video_not_found",
            "The selected video does not exist or is not a local file.",
        ));
    }

    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| {
            VideoInspectError::new(
                "unsupported_video_format",
                "The file has no video extension.",
            )
        })?;
    if !ACCEPTED_VIDEO_EXTENSIONS.contains(&extension.as_str()) {
        return Err(VideoInspectError::new(
            "unsupported_video_format",
            format!("The .{extension} format is not supported."),
        ));
    }

    let metadata = fs::metadata(path).map_err(|error| {
        VideoInspectError::new(
            "video_read_error",
            format!("Could not read the video: {error}"),
        )
    })?;
    if metadata.len() == 0 {
        return Err(VideoInspectError::new(
            "invalid_video",
            "The selected video is empty.",
        ));
    }

    let probe = run_tool(
        ffprobe,
        library_dir,
        [
            OsString::from("-v"),
            OsString::from("error"),
            OsString::from("-print_format"),
            OsString::from("json"),
            OsString::from("-show_format"),
            OsString::from("-show_streams"),
            path.as_os_str().to_owned(),
        ],
    )
    .map_err(|error| VideoInspectError::new("ffprobe_unavailable", error))?;

    if !probe.status.success() {
        let detail = String::from_utf8_lossy(&probe.stderr).trim().to_owned();
        return Err(VideoInspectError::new(
            "invalid_video",
            if detail.is_empty() {
                "FFprobe could not read this file.".to_owned()
            } else {
                detail
            },
        ));
    }

    let document: serde_json::Value = serde_json::from_slice(&probe.stdout).map_err(|error| {
        VideoInspectError::new(
            "invalid_video",
            format!("FFprobe returned invalid metadata: {error}"),
        )
    })?;
    let streams = document["streams"].as_array().ok_or_else(|| {
        VideoInspectError::new("invalid_video", "FFprobe found no media streams.")
    })?;
    let video = streams
        .iter()
        .find(|stream| stream["codec_type"].as_str() == Some("video"))
        .ok_or_else(|| {
            VideoInspectError::new(
                "unsupported_video_file",
                "The selected file contains no video stream.",
            )
        })?;

    let mut width = video["width"].as_u64().unwrap_or(0);
    let mut height = video["height"].as_u64().unwrap_or(0);
    if width == 0 || height == 0 {
        return Err(VideoInspectError::new(
            "invalid_video",
            "The video resolution is missing or invalid.",
        ));
    }
    if display_rotation(video).unsigned_abs() % 180 == 90 {
        std::mem::swap(&mut width, &mut height);
    }

    let duration_seconds = document["format"]["duration"]
        .as_str()
        .or_else(|| video["duration"].as_str())
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(0.0);
    let video_codec = video["codec_name"]
        .as_str()
        .unwrap_or("unknown")
        .to_ascii_uppercase();
    let audio_codec = streams
        .iter()
        .find(|stream| stream["codec_type"].as_str() == Some("audio"))
        .and_then(|stream| stream["codec_name"].as_str())
        .map(str::to_ascii_uppercase);
    let thumbnail_data = extract_thumbnail(path, ffmpeg, library_dir, duration_seconds);

    Ok(VideoInfo {
        path: path.to_string_lossy().into_owned(),
        name: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("video")
            .to_owned(),
        format: match extension.as_str() {
            "webm" => "WebM".to_owned(),
            value => value.to_ascii_uppercase(),
        },
        duration_seconds,
        width,
        height,
        size: metadata.len(),
        video_codec,
        audio_codec,
        thumbnail_data,
    })
}

fn display_rotation(stream: &serde_json::Value) -> i64 {
    stream["tags"]["rotate"]
        .as_str()
        .and_then(|value| value.parse::<i64>().ok())
        .or_else(|| {
            stream["side_data_list"]
                .as_array()
                .and_then(|items| items.iter().find_map(|item| item["rotation"].as_i64()))
        })
        .unwrap_or(0)
}

fn extract_thumbnail(
    path: &Path,
    ffmpeg: &Path,
    library_dir: &Path,
    duration_seconds: f64,
) -> Option<Vec<u8>> {
    let seek = if duration_seconds > 0.0 {
        (duration_seconds * 0.1).min(5.0)
    } else {
        0.0
    };
    let output = run_tool(
        ffmpeg,
        library_dir,
        [
            OsString::from("-v"),
            OsString::from("error"),
            OsString::from("-ss"),
            OsString::from(format!("{seek:.3}")),
            OsString::from("-i"),
            path.as_os_str().to_owned(),
            OsString::from("-map"),
            OsString::from("0:v:0"),
            OsString::from("-frames:v"),
            OsString::from("1"),
            OsString::from("-vf"),
            OsString::from("scale=240:-2"),
            OsString::from("-f"),
            OsString::from("image2pipe"),
            OsString::from("-vcodec"),
            OsString::from("png"),
            OsString::from("pipe:1"),
        ],
    )
    .ok()?;
    (output.status.success() && !output.stdout.is_empty() && output.stdout.len() <= 2_000_000)
        .then_some(output.stdout)
}

fn run_tool<I>(binary: &Path, library_dir: &Path, args: I) -> Result<Output, String>
where
    I: IntoIterator<Item = OsString>,
{
    Command::new(binary)
        .args(args)
        .env("PATH", path_with_runtime_libraries(library_dir))
        .output()
        .map_err(|error| format!("could not start {}: {error}", binary.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundled_tool(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!("{name}-{TARGET_TRIPLE}.exe"))
    }

    #[test]
    fn bundled_video_tools_are_lgpl_and_have_the_expected_version() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let library_dir = manifest_dir.join("resources/ffmpeg");

        for name in ["ffmpeg", "ffprobe"] {
            let binary = bundled_tool(name);
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

    fn create_test_video(path: &Path, format: &str) {
        let library_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/ffmpeg");
        let codec = if format == "mp4" { "mpeg4" } else { "mpeg4" };
        let output = run_tool(
            &bundled_tool("ffmpeg"),
            &library_dir,
            [
                OsString::from("-v"),
                OsString::from("error"),
                OsString::from("-y"),
                OsString::from("-f"),
                OsString::from("lavfi"),
                OsString::from("-i"),
                OsString::from("testsrc=size=320x180:rate=5"),
                OsString::from("-f"),
                OsString::from("lavfi"),
                OsString::from("-i"),
                OsString::from("sine=frequency=880:sample_rate=44100"),
                OsString::from("-t"),
                OsString::from("1"),
                OsString::from("-c:v"),
                OsString::from(codec),
                OsString::from("-c:a"),
                OsString::from(if format == "mp4" { "aac" } else { "mp3" }),
                OsString::from("-shortest"),
                path.as_os_str().to_owned(),
            ],
        )
        .expect("test fixture generation must start");
        assert!(
            output.status.success(),
            "fixture generation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn analyzes_real_mp4_and_avi_files_with_ffprobe() {
        let temp = tempfile::tempdir().unwrap();
        let library_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/ffmpeg");

        for format in ["mp4", "avi"] {
            let path = temp.path().join(format!("sample.{format}"));
            create_test_video(&path, format);
            let info = inspect_video_with_tools(
                &path,
                &bundled_tool("ffprobe"),
                &bundled_tool("ffmpeg"),
                &library_dir,
            )
            .unwrap();

            assert_eq!((info.width, info.height), (320, 180));
            assert_eq!(info.format, format.to_ascii_uppercase());
            assert!(info.duration_seconds >= 0.9);
            assert_eq!(info.video_codec, "MPEG4");
            assert!(info.audio_codec.is_some());
            assert!(info.thumbnail_data.is_some());
        }
    }

    #[test]
    fn rejects_a_corrupted_supported_video() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("corrupted.mp4");
        fs::write(&path, b"not a video").unwrap();
        let library_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/ffmpeg");
        let error = inspect_video_with_tools(
            &path,
            &bundled_tool("ffprobe"),
            &bundled_tool("ffmpeg"),
            &library_dir,
        )
        .unwrap_err();
        assert_eq!(error.code, "invalid_video");
    }
}
