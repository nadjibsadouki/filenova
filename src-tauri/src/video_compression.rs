use std::{
    collections::HashSet,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, Window};

use crate::video_tools::{
    find_sidecar, inspect_video_with_tools, path_with_runtime_libraries, runtime_library_dir,
};

const RECOMMENDED_QUALITY: u8 = 24;
const WEBM_CRF: u8 = 34;
const WEBM_PRESET: u8 = 8;

#[derive(Clone, Default)]
pub struct VideoCompressionState {
    inner: Arc<VideoCompressionInner>,
}

#[derive(Default)]
struct VideoCompressionInner {
    child: Mutex<Option<Child>>,
    cancel_requested: AtomicBool,
    completed_results: Mutex<HashSet<PathBuf>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoCompressionRequest {
    path: String,
    destination_dir: Option<String>,
    mode: String,
    preserve_resolution: bool,
    remove_audio: bool,
    preserve_metadata: bool,
    output_format: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoCompressionProgress {
    path: String,
    name: String,
    processed_seconds: f64,
    duration_seconds: f64,
    percent: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoCompressionResult {
    original_size: u64,
    compressed_size: u64,
    saved_bytes: u64,
    reduction_percent: f64,
    duration_seconds: f64,
    width: u64,
    height: u64,
    video_codec: String,
    audio_codec: Option<String>,
    output_path: String,
    output_name: String,
    no_size_reduction: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoCompressionError {
    code: &'static str,
    message: String,
}

impl VideoCompressionError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[tauri::command]
pub async fn compress_recommended_video(
    app: AppHandle,
    window: Window,
    state: State<'_, VideoCompressionState>,
    request: VideoCompressionRequest,
) -> Result<VideoCompressionResult, VideoCompressionError> {
    let ffmpeg = find_sidecar(&app, "ffmpeg")
        .map_err(|error| VideoCompressionError::new("ffmpeg_unavailable", error))?;
    let ffprobe = find_sidecar(&app, "ffprobe")
        .map_err(|error| VideoCompressionError::new("ffprobe_unavailable", error))?;
    let library_dir = runtime_library_dir(&app);
    let compression_state = state.inner().clone();
    let progress_window = window.clone();

    let result = tauri::async_runtime::spawn_blocking(move || {
        compress_with_tools(
            request,
            &ffmpeg,
            &ffprobe,
            &library_dir,
            &compression_state,
            |progress| {
                let _ = progress_window.emit("video-compression-progress", progress);
            },
        )
    })
    .await
    .map_err(|error| VideoCompressionError::new("encoding_failed", error.to_string()))??;

    let output_path = PathBuf::from(&result.output_path);
    app.asset_protocol_scope()
        .allow_file(&output_path)
        .map_err(|error| VideoCompressionError::new("output_invalid", error.to_string()))?;
    state
        .inner
        .completed_results
        .lock()
        .map_err(|_| VideoCompressionError::new("encoding_failed", "Result lock is poisoned."))?
        .insert(output_path);

    Ok(result)
}

#[tauri::command]
pub fn cancel_video_compression(
    state: State<'_, VideoCompressionState>,
) -> Result<bool, VideoCompressionError> {
    let mut active =
        state.inner.child.lock().map_err(|_| {
            VideoCompressionError::new("encoding_failed", "Process lock is poisoned.")
        })?;
    let Some(child) = active.as_mut() else {
        return Ok(false);
    };
    state.inner.cancel_requested.store(true, Ordering::SeqCst);
    child
        .kill()
        .map_err(|error| VideoCompressionError::new("cancel_failed", error.to_string()))?;
    Ok(true)
}

#[tauri::command]
pub fn delete_video_result(
    state: State<'_, VideoCompressionState>,
    path: String,
) -> Result<(), VideoCompressionError> {
    let path = PathBuf::from(path);
    let mut results = state
        .inner
        .completed_results
        .lock()
        .map_err(|_| VideoCompressionError::new("delete_failed", "Result lock is poisoned."))?;
    if !results.contains(&path) {
        return Err(VideoCompressionError::new(
            "delete_not_allowed",
            "Only results created during this FileNova session can be deleted.",
        ));
    }
    fs::remove_file(&path)
        .map_err(|error| VideoCompressionError::new("delete_failed", error.to_string()))?;
    results.remove(&path);
    Ok(())
}

fn compress_with_tools<F>(
    request: VideoCompressionRequest,
    ffmpeg: &Path,
    ffprobe: &Path,
    library_dir: &Path,
    state: &VideoCompressionState,
    mut report: F,
) -> Result<VideoCompressionResult, VideoCompressionError>
where
    F: FnMut(VideoCompressionProgress),
{
    if request.mode != "recommended" {
        return Err(VideoCompressionError::new(
            "mode_not_implemented",
            "Only Recommended mode is implemented.",
        ));
    }
    let output_extension = match request.output_format.as_str() {
        "mp4" => "mp4",
        "webm" => "webm",
        _ => {
            return Err(VideoCompressionError::new(
                "invalid_output_format",
                "The output format must be MP4 or WebM.",
            ))
        }
    };

    let input_path = PathBuf::from(&request.path);
    let input = inspect_video_with_tools(&input_path, ffprobe, ffmpeg, library_dir)
        .map_err(|error| VideoCompressionError::new(error.code, error.message))?;
    if input.duration_seconds <= 0.0 {
        return Err(VideoCompressionError::new(
            "invalid_video",
            "The video duration is missing or invalid.",
        ));
    }

    let destination = destination_directory(&input_path, request.destination_dir.as_deref())?;
    let stem = input_path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("video");
    let base_name = format!("{stem}-compressed");
    let initial_output = unique_output_path(&destination, &base_name, output_extension);
    let temporary = temporary_output_path(&destination, &base_name, output_extension);
    verify_destination_writable(&temporary)?;

    let args = recommended_ffmpeg_args(&request, &input_path, &temporary);
    let mut command = Command::new(ffmpeg);
    command
        .args(&args)
        .env("PATH", path_with_runtime_libraries(library_dir))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }

    state.inner.cancel_requested.store(false, Ordering::SeqCst);
    let mut child = command.spawn().map_err(|error| {
        let _ = fs::remove_file(&temporary);
        VideoCompressionError::new("ffmpeg_unavailable", error.to_string())
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        VideoCompressionError::new("encoding_failed", "FFmpeg progress pipe is unavailable.")
    })?;
    let mut stderr = child.stderr.take().ok_or_else(|| {
        VideoCompressionError::new("encoding_failed", "FFmpeg error pipe is unavailable.")
    })?;

    {
        let mut active = state.inner.child.lock().map_err(|_| {
            VideoCompressionError::new("encoding_failed", "Process lock is poisoned.")
        })?;
        if active.is_some() {
            let _ = child.kill();
            let _ = fs::remove_file(&temporary);
            return Err(VideoCompressionError::new(
                "compression_busy",
                "Another video compression is already running.",
            ));
        }
        *active = Some(child);
    }

    let stderr_reader = thread::spawn(move || {
        let mut message = String::new();
        let _ = stderr.read_to_string(&mut message);
        message
    });

    let input_name = input.name.clone();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(value) = line.strip_prefix("out_time_us=") {
            if let Ok(microseconds) = value.parse::<f64>() {
                let processed = (microseconds / 1_000_000.0).max(0.0);
                let percent = ((processed / input.duration_seconds) * 100.0).clamp(0.0, 99.9);
                report(VideoCompressionProgress {
                    path: request.path.clone(),
                    name: input_name.clone(),
                    processed_seconds: processed,
                    duration_seconds: input.duration_seconds,
                    percent,
                });
            }
        }
    }

    let status = state
        .inner
        .child
        .lock()
        .map_err(|_| VideoCompressionError::new("encoding_failed", "Process lock is poisoned."))?
        .take()
        .ok_or_else(|| {
            VideoCompressionError::new("encoding_failed", "FFmpeg process disappeared.")
        })?
        .wait()
        .map_err(|error| VideoCompressionError::new("encoding_failed", error.to_string()))?;
    let stderr_message = stderr_reader.join().unwrap_or_default();
    let cancelled = state.inner.cancel_requested.swap(false, Ordering::SeqCst);

    if cancelled {
        let _ = fs::remove_file(&temporary);
        return Err(VideoCompressionError::new(
            "compression_cancelled",
            "Video compression was cancelled.",
        ));
    }
    if !status.success() {
        let _ = fs::remove_file(&temporary);
        return Err(classify_ffmpeg_error(&stderr_message));
    }

    let temporary_size = fs::metadata(&temporary)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if temporary_size == 0 {
        let _ = fs::remove_file(&temporary);
        return Err(VideoCompressionError::new(
            "output_empty",
            "FFmpeg created an empty output file.",
        ));
    }

    let verified =
        inspect_video_with_tools(&temporary, ffprobe, ffmpeg, library_dir).map_err(|error| {
            let _ = fs::remove_file(&temporary);
            VideoCompressionError::new("output_invalid", error.message)
        })?;
    let final_output = if initial_output.exists() {
        unique_output_path(&destination, &base_name, output_extension)
    } else {
        initial_output
    };
    fs::rename(&temporary, &final_output).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        VideoCompressionError::new("destination_inaccessible", error.to_string())
    })?;

    let compressed_size = fs::metadata(&final_output)
        .map_err(|error| VideoCompressionError::new("output_invalid", error.to_string()))?
        .len();
    if compressed_size == 0 {
        let _ = fs::remove_file(&final_output);
        return Err(VideoCompressionError::new(
            "output_empty",
            "The final output file is empty.",
        ));
    }

    let saved_bytes = input.size.saturating_sub(compressed_size);
    let no_size_reduction = compressed_size >= input.size;
    let reduction_percent = if no_size_reduction || input.size == 0 {
        0.0
    } else {
        (saved_bytes as f64 / input.size as f64) * 100.0
    };
    report(VideoCompressionProgress {
        path: request.path,
        name: input_name,
        processed_seconds: input.duration_seconds,
        duration_seconds: input.duration_seconds,
        percent: 100.0,
    });

    Ok(VideoCompressionResult {
        original_size: input.size,
        compressed_size,
        saved_bytes,
        reduction_percent,
        duration_seconds: verified.duration_seconds,
        width: verified.width,
        height: verified.height,
        video_codec: verified.video_codec,
        audio_codec: verified.audio_codec,
        output_name: final_output
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("video-compressed")
            .to_owned(),
        output_path: final_output.to_string_lossy().into_owned(),
        no_size_reduction,
    })
}

fn destination_directory(
    input: &Path,
    requested: Option<&str>,
) -> Result<PathBuf, VideoCompressionError> {
    let destination = if let Some(requested) = requested {
        let path = PathBuf::from(requested);
        if !path.is_absolute() || !path.is_dir() {
            return Err(VideoCompressionError::new(
                "destination_inaccessible",
                "The selected destination folder is unavailable.",
            ));
        }
        path
    } else {
        let parent = input.parent().ok_or_else(|| {
            VideoCompressionError::new(
                "destination_inaccessible",
                "The input has no parent folder.",
            )
        })?;
        let path = parent.join("FileNova Compressed");
        fs::create_dir_all(&path).map_err(|error| {
            VideoCompressionError::new("destination_inaccessible", error.to_string())
        })?;
        path
    };
    Ok(destination)
}

fn unique_output_path(directory: &Path, base: &str, extension: &str) -> PathBuf {
    let first = directory.join(format!("{base}.{extension}"));
    if !first.exists() {
        return first;
    }
    for suffix in 2..=10_000 {
        let candidate = directory.join(format!("{base}-{suffix}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    directory.join(format!("{base}-{}.{}", std::process::id(), extension))
}

fn temporary_output_path(directory: &Path, base: &str, extension: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    directory.join(format!(
        ".{base}.filenova-{}-{nonce}.tmp.{extension}",
        std::process::id()
    ))
}

fn verify_destination_writable(path: &Path) -> Result<(), VideoCompressionError> {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => {
            drop(file);
            fs::remove_file(path).map_err(|error| {
                VideoCompressionError::new("destination_inaccessible", error.to_string())
            })
        }
        Err(error) => Err(VideoCompressionError::new(
            "destination_inaccessible",
            error.to_string(),
        )),
    }
}

fn recommended_ffmpeg_args(
    request: &VideoCompressionRequest,
    input: &Path,
    output: &Path,
) -> Vec<OsString> {
    let mut args = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-v".into(),
        "warning".into(),
        "-n".into(),
        "-i".into(),
        input.as_os_str().to_owned(),
        "-map".into(),
        "0:v:0".into(),
    ];
    if request.remove_audio {
        args.push("-an".into());
    } else {
        args.extend(["-map".into(), "0:a:0?".into()]);
    }
    if request.preserve_metadata {
        args.extend(["-map_metadata".into(), "0".into()]);
    } else {
        args.extend([
            "-map_metadata".into(),
            "-1".into(),
            "-map_chapters".into(),
            "-1".into(),
        ]);
    }
    if !request.preserve_resolution {
        args.extend([
            "-vf".into(),
            "scale=min(1920\\,iw):min(1080\\,ih):force_original_aspect_ratio=decrease:force_divisible_by=2".into(),
        ]);
    }

    match request.output_format.as_str() {
        "mp4" => {
            // libx264 is intentionally absent from the LGPL build. OpenH264 quality mode with
            // global_quality 24 is the closest available equivalent to CRF 24; it has no presets.
            args.extend([
                "-c:v".into(),
                "libopenh264".into(),
                "-rc_mode".into(),
                "quality".into(),
                "-global_quality".into(),
                RECOMMENDED_QUALITY.to_string().into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
            ]);
            if !request.remove_audio {
                args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "160k".into()]);
            }
            args.extend(["-movflags".into(), "+faststart".into()]);
        }
        "webm" => {
            args.extend([
                "-c:v".into(),
                "libsvtav1".into(),
                "-crf".into(),
                WEBM_CRF.to_string().into(),
                "-preset".into(),
                WEBM_PRESET.to_string().into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
            ]);
            if !request.remove_audio {
                args.extend([
                    "-c:a".into(),
                    "libopus".into(),
                    "-b:a".into(),
                    "128k".into(),
                ]);
            }
        }
        _ => {}
    }
    args.extend([
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        output.as_os_str().to_owned(),
    ]);
    args
}

fn classify_ffmpeg_error(stderr: &str) -> VideoCompressionError {
    let detail = stderr.trim();
    let lower = detail.to_ascii_lowercase();
    let code = if lower.contains("no space left")
        || lower.contains("not enough space")
        || lower.contains("disk full")
    {
        "disk_full"
    } else if lower.contains("permission denied") || lower.contains("access is denied") {
        "destination_inaccessible"
    } else if lower.contains("unknown encoder")
        || lower.contains("unsupported codec")
        || lower.contains("error while opening encoder")
    {
        "unsupported_codec"
    } else {
        "encoding_failed"
    };
    VideoCompressionError::new(
        code,
        if detail.is_empty() {
            "FFmpeg failed without an error message.".to_owned()
        } else {
            detail.to_owned()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video_tools::TARGET_TRIPLE;

    fn bundled_tool(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!("{name}-{TARGET_TRIPLE}.exe"))
    }

    fn library_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/ffmpeg")
    }

    fn create_source(path: &Path, format: &str) {
        let mut command = Command::new(bundled_tool("ffmpeg"));
        command
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=12",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=700:sample_rate=44100",
                "-t",
                "1",
                "-c:v",
                "mpeg4",
                "-c:a",
                if format == "mp4" { "aac" } else { "mp3" },
                "-shortest",
            ])
            .arg(path)
            .env("PATH", path_with_runtime_libraries(&library_dir()));
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn create_long_source(path: &Path) {
        let output = Command::new(bundled_tool("ffmpeg"))
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=1280x720:rate=30",
                "-t",
                "8",
                "-c:v",
                "mpeg4",
                "-an",
            ])
            .arg(path)
            .env("PATH", path_with_runtime_libraries(&library_dir()))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn request(path: &Path, destination: &Path, output_format: &str) -> VideoCompressionRequest {
        VideoCompressionRequest {
            path: path.to_string_lossy().into_owned(),
            destination_dir: Some(destination.to_string_lossy().into_owned()),
            mode: "recommended".into(),
            preserve_resolution: true,
            remove_audio: false,
            preserve_metadata: true,
            output_format: output_format.into(),
        }
    }

    #[test]
    fn recommended_arguments_implement_supported_options_without_changing_framerate() {
        let input = Path::new("C:\\input.mp4");
        let output = Path::new("C:\\output.mp4");
        let mut req = request(input, Path::new("C:\\"), "mp4");
        req.preserve_metadata = false;
        let joined = recommended_ffmpeg_args(&req, input, output)
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains("-c:v libopenh264"));
        assert!(joined.contains("-global_quality 24"));
        assert!(joined.contains("-pix_fmt yuv420p"));
        assert!(joined.contains("-c:a aac -b:a 160k"));
        assert!(joined.contains("-movflags +faststart"));
        assert!(joined.contains("-map_metadata -1 -map_chapters -1"));
        assert!(!joined.contains(" -r "));
        assert!(!joined.contains("scale="));

        req.output_format = "webm".into();
        req.preserve_resolution = false;
        req.remove_audio = true;
        let joined = recommended_ffmpeg_args(&req, input, Path::new("C:\\output.webm"))
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains("-c:v libsvtav1 -crf 34 -preset 8"));
        assert!(joined.contains("scale="));
        assert!(joined.contains("-an"));
        assert!(!joined.contains("libopus"));
    }

    #[test]
    fn compresses_real_mp4_and_avi_sequentially_and_never_overwrites() {
        let temp = tempfile::tempdir().unwrap();
        let output_dir = temp.path().join("outputs");
        fs::create_dir(&output_dir).unwrap();
        let mp4 = temp.path().join("video.mp4");
        let avi = temp.path().join("clip.avi");
        create_source(&mp4, "mp4");
        create_source(&avi, "avi");
        let mp4_original = fs::read(&mp4).unwrap();
        let avi_original = fs::read(&avi).unwrap();
        let state = VideoCompressionState::default();

        let mut progress = Vec::new();
        let first = compress_with_tools(
            request(&mp4, &output_dir, "mp4"),
            &bundled_tool("ffmpeg"),
            &bundled_tool("ffprobe"),
            &library_dir(),
            &state,
            |event| progress.push(event.percent),
        )
        .unwrap();
        let second = compress_with_tools(
            request(&avi, &output_dir, "webm"),
            &bundled_tool("ffmpeg"),
            &bundled_tool("ffprobe"),
            &library_dir(),
            &state,
            |_| {},
        )
        .unwrap();
        let duplicate = compress_with_tools(
            request(&mp4, &output_dir, "mp4"),
            &bundled_tool("ffmpeg"),
            &bundled_tool("ffprobe"),
            &library_dir(),
            &state,
            |_| {},
        )
        .unwrap();

        assert_eq!(first.video_codec, "H264");
        assert_eq!(first.audio_codec.as_deref(), Some("AAC"));
        assert_eq!(second.video_codec, "AV1");
        assert_eq!(second.audio_codec.as_deref(), Some("OPUS"));
        assert_eq!(first.output_name, "video-compressed.mp4");
        assert_eq!(duplicate.output_name, "video-compressed-2.mp4");
        assert!(first.compressed_size > 0 && second.compressed_size > 0);
        assert!(progress.iter().any(|percent| *percent > 0.0));
        assert_eq!(progress.last().copied(), Some(100.0));
        for result in [&first, &second, &duplicate] {
            if result.no_size_reduction {
                assert_eq!(result.saved_bytes, 0);
                assert_eq!(result.reduction_percent, 0.0);
            }
        }
        assert_eq!(fs::read(&mp4).unwrap(), mp4_original);
        assert_eq!(fs::read(&avi).unwrap(), avi_original);
    }

    #[test]
    fn cancellation_kills_only_ffmpeg_and_removes_the_temporary_output() {
        let temp = tempfile::tempdir().unwrap();
        let output_dir = temp.path().join("outputs");
        fs::create_dir(&output_dir).unwrap();
        let source = temp.path().join("long.mp4");
        create_long_source(&source);
        let original = fs::read(&source).unwrap();
        let state = VideoCompressionState::default();
        let worker_state = state.clone();
        let worker_source = source.clone();
        let worker_output = output_dir.clone();

        let worker = thread::spawn(move || {
            compress_with_tools(
                request(&worker_source, &worker_output, "webm"),
                &bundled_tool("ffmpeg"),
                &bundled_tool("ffprobe"),
                &library_dir(),
                &worker_state,
                |_| {},
            )
        });

        let mut killed = false;
        for _ in 0..500 {
            if let Ok(mut active) = state.inner.child.lock() {
                if let Some(child) = active.as_mut() {
                    state.inner.cancel_requested.store(true, Ordering::SeqCst);
                    child.kill().unwrap();
                    killed = true;
                    break;
                }
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(killed, "FFmpeg did not become active in time");
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error.code, "compression_cancelled");
        assert_eq!(fs::read(&source).unwrap(), original);
        assert!(fs::read_dir(&output_dir).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp.")));
    }
}
