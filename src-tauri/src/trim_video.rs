use std::{
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, Window};

use crate::{
    trim_video_plan::validate_trim_selection,
    video_tools::{
        find_sidecar, inspect_video_with_tools, path_with_runtime_libraries, runtime_library_dir,
    },
};

#[derive(Clone, Default)]
pub struct TrimVideoState {
    inner: Arc<TrimVideoInner>,
}

#[derive(Default)]
struct TrimVideoInner {
    child: Mutex<Option<Child>>,
    cancel_requested: AtomicBool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FastTrimRequest {
    path: String,
    destination_dir: Option<String>,
    preserve_original_format: bool,
    output_format: String,
    preserve_metadata: bool,
    remove_audio: bool,
    output_name: String,
    start_seconds: f64,
    end_seconds: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FastTrimProgress {
    path: String,
    name: String,
    processed_seconds: f64,
    duration_seconds: f64,
    percent: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FastTrimResult {
    requested_start_seconds: f64,
    requested_end_seconds: f64,
    requested_duration_seconds: f64,
    detected_actual_start_seconds: f64,
    final_duration_seconds: f64,
    keyframe_offset_seconds: f64,
    final_format: String,
    final_video_codec: String,
    final_audio_codec: Option<String>,
    original_size: u64,
    final_size: u64,
    processing_duration_seconds: f64,
    output_path: String,
    output_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FastTrimError {
    code: &'static str,
    message: String,
}

impl FastTrimError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[tauri::command]
pub async fn fast_trim_video(
    app: AppHandle,
    window: Window,
    state: State<'_, TrimVideoState>,
    request: FastTrimRequest,
) -> Result<FastTrimResult, FastTrimError> {
    let ffmpeg = find_sidecar(&app, "ffmpeg")
        .map_err(|error| FastTrimError::new("ffmpeg_unavailable", error))?;
    let ffprobe = find_sidecar(&app, "ffprobe")
        .map_err(|error| FastTrimError::new("ffprobe_unavailable", error))?;
    let library_dir = runtime_library_dir(&app);
    let trim_state = state.inner().clone();
    let progress_window = window.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        fast_trim_with_tools(
            request,
            &ffmpeg,
            &ffprobe,
            &library_dir,
            &trim_state,
            |progress| {
                let _ = progress_window.emit("fast-trim-progress", progress);
            },
        )
    })
    .await
    .map_err(|error| FastTrimError::new("trim_failed", error.to_string()))??;
    app.asset_protocol_scope()
        .allow_file(Path::new(&result.output_path))
        .map_err(|error| FastTrimError::new("output_invalid", error.to_string()))?;
    Ok(result)
}

#[tauri::command]
pub fn cancel_fast_trim(state: State<'_, TrimVideoState>) -> Result<bool, FastTrimError> {
    let mut active = state
        .inner
        .child
        .lock()
        .map_err(|_| FastTrimError::new("trim_failed", "Process lock poisoned."))?;
    let Some(child) = active.as_mut() else {
        return Ok(false);
    };
    state.inner.cancel_requested.store(true, Ordering::SeqCst);
    child
        .kill()
        .map_err(|error| FastTrimError::new("cancel_failed", error.to_string()))?;
    Ok(true)
}

pub(crate) fn video_copy_compatible(container: &str, codec: &str) -> bool {
    let codec = codec.to_ascii_uppercase().replace('-', "");
    match container.to_ascii_lowercase().as_str() {
        "mp4" | "m4v" => ["H264", "HEVC", "AV1", "MPEG4"].contains(&codec.as_str()),
        "mov" => ["H264", "HEVC", "MPEG4", "PRORES"].contains(&codec.as_str()),
        "webm" => ["VP8", "VP9", "AV1"].contains(&codec.as_str()),
        "mkv" | "avi" => !codec.is_empty() && codec != "UNKNOWN",
        _ => false,
    }
}

pub(crate) fn audio_copy_compatible(container: &str, codec: Option<&str>) -> bool {
    let Some(codec) = codec else {
        return true;
    };
    let codec = codec.to_ascii_uppercase().replace('-', "");
    match container.to_ascii_lowercase().as_str() {
        "mp4" | "m4v" | "mov" => ["AAC", "MP3", "ALAC", "OPUS"].contains(&codec.as_str()),
        "webm" => ["OPUS", "VORBIS"].contains(&codec.as_str()),
        "mkv" | "avi" => !codec.is_empty() && codec != "UNKNOWN",
        _ => false,
    }
}

fn fast_trim_with_tools<F>(
    request: FastTrimRequest,
    ffmpeg: &Path,
    ffprobe: &Path,
    library_dir: &Path,
    state: &TrimVideoState,
    mut report: F,
) -> Result<FastTrimResult, FastTrimError>
where
    F: FnMut(FastTrimProgress),
{
    let started_at = Instant::now();
    let input_path = PathBuf::from(&request.path);
    let input = inspect_video_with_tools(&input_path, ffprobe, ffmpeg, library_dir)
        .map_err(|error| FastTrimError::new(error.code, error.message))?;
    validate_trim_selection(
        request.start_seconds,
        request.end_seconds,
        input.duration_seconds,
    )
    .map_err(|reason| FastTrimError::new("invalid_trim_selection", reason))?;
    let source_extension = input_path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| FastTrimError::new("container_incompatible", "Missing source container."))?;
    let container = if request.preserve_original_format {
        source_extension
    } else {
        request.output_format.to_ascii_lowercase()
    };
    if !matches!(
        container.as_str(),
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v"
    ) {
        return Err(FastTrimError::new(
            "container_incompatible",
            "The selected output container is not supported by Fast Cut.",
        ));
    }
    if !video_copy_compatible(&container, &input.video_codec) {
        return Err(FastTrimError::new(
            "fast_cut_video_incompatible",
            "The source video codec cannot be copied into the selected container. Use Precise Cut later.",
        ));
    }
    if !request.remove_audio && !audio_copy_compatible(&container, input.audio_codec.as_deref()) {
        return Err(FastTrimError::new(
            "fast_cut_audio_incompatible",
            "The source audio codec cannot be copied into the selected container. Use Precise Cut later.",
        ));
    }
    let destination = destination_directory(&input_path, request.destination_dir.as_deref())?;
    let base_name = sanitized_output_name(&request.output_name, &input_path)?;
    let temporary = temporary_output_path(&destination, &base_name, &container);
    verify_destination_writable(&temporary)?;
    let mut cleanup = TemporaryOutput(Some(temporary.clone()));
    let requested_duration = request.end_seconds - request.start_seconds;
    let detected_actual_start =
        detect_previous_keyframe(&input_path, ffprobe, library_dir, request.start_seconds)
            .unwrap_or(request.start_seconds);
    let args = fast_cut_args(
        &request,
        &input_path,
        &temporary,
        requested_duration,
        &container,
    );
    report(FastTrimProgress {
        path: request.path.clone(),
        name: input.name.clone(),
        processed_seconds: 0.0,
        duration_seconds: requested_duration,
        percent: 0.0,
    });
    run_ffmpeg(
        ffmpeg,
        library_dir,
        state,
        &args,
        &request.path,
        &input.name,
        requested_duration,
        &mut report,
    )?;
    let verified = validate_trim_output(
        &temporary,
        ffprobe,
        library_dir,
        requested_duration,
        request.remove_audio,
    )?;
    let final_output = unique_output_path(&destination, &base_name, &container);
    fs::rename(&temporary, &final_output)
        .map_err(|error| FastTrimError::new("destination_inaccessible", error.to_string()))?;
    cleanup.0 = None;
    let final_size = fs::metadata(&final_output)
        .map_err(|error| FastTrimError::new("output_invalid", error.to_string()))?
        .len();
    report(FastTrimProgress {
        path: request.path,
        name: input.name,
        processed_seconds: requested_duration,
        duration_seconds: requested_duration,
        percent: 100.0,
    });
    Ok(FastTrimResult {
        requested_start_seconds: request.start_seconds,
        requested_end_seconds: request.end_seconds,
        requested_duration_seconds: requested_duration,
        detected_actual_start_seconds: detected_actual_start,
        final_duration_seconds: verified.duration_seconds,
        keyframe_offset_seconds: detected_actual_start - request.start_seconds,
        final_format: if container == "webm" {
            "WebM".into()
        } else {
            container.to_ascii_uppercase()
        },
        final_video_codec: verified.video_codec,
        final_audio_codec: verified.audio_codec,
        original_size: input.size,
        final_size,
        processing_duration_seconds: started_at.elapsed().as_secs_f64(),
        output_name: final_output
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("video-trimmed")
            .to_owned(),
        output_path: final_output.to_string_lossy().into_owned(),
    })
}

fn fast_cut_args(
    request: &FastTrimRequest,
    input: &Path,
    output: &Path,
    duration: f64,
    container: &str,
) -> Vec<OsString> {
    let mut args = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-v".into(),
        "warning".into(),
        "-n".into(),
        "-ss".into(),
        format!("{:.6}", request.start_seconds).into(),
        "-i".into(),
        input.as_os_str().to_owned(),
        "-t".into(),
        format!("{duration:.6}").into(),
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
    args.extend([
        "-c".into(),
        "copy".into(),
        "-avoid_negative_ts".into(),
        "make_zero".into(),
    ]);
    if matches!(container, "mp4" | "m4v") {
        args.extend(["-movflags".into(), "+faststart".into()]);
    }
    args.extend([
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        output.as_os_str().to_owned(),
    ]);
    args
}

#[allow(clippy::too_many_arguments)]
fn run_ffmpeg<F>(
    ffmpeg: &Path,
    library_dir: &Path,
    state: &TrimVideoState,
    args: &[OsString],
    input_path: &str,
    input_name: &str,
    duration: f64,
    report: &mut F,
) -> Result<(), FastTrimError>
where
    F: FnMut(FastTrimProgress),
{
    let mut command = Command::new(ffmpeg);
    command
        .args(args)
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
    let mut child = command
        .spawn()
        .map_err(|error| FastTrimError::new("ffmpeg_unavailable", error.to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| FastTrimError::new("trim_failed", "FFmpeg progress is unavailable."))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| FastTrimError::new("trim_failed", "FFmpeg error output is unavailable."))?;
    {
        let mut active = state
            .inner
            .child
            .lock()
            .map_err(|_| FastTrimError::new("trim_failed", "Process lock poisoned."))?;
        if active.is_some() {
            let _ = child.kill();
            return Err(FastTrimError::new(
                "trim_busy",
                "Another Fast Cut is already running.",
            ));
        }
        *active = Some(child);
    }
    let stderr_reader = thread::spawn(move || {
        let mut message = String::new();
        let _ = stderr.read_to_string(&mut message);
        message
    });
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(value) = line.strip_prefix("out_time_us=") {
            if let Ok(microseconds) = value.parse::<f64>() {
                let processed = (microseconds / 1_000_000.0).max(0.0);
                report(FastTrimProgress {
                    path: input_path.into(),
                    name: input_name.into(),
                    processed_seconds: processed,
                    duration_seconds: duration,
                    percent: (processed / duration * 100.0).clamp(0.0, 99.9),
                });
            }
        }
    }
    let status = state
        .inner
        .child
        .lock()
        .map_err(|_| FastTrimError::new("trim_failed", "Process lock poisoned."))?
        .as_mut()
        .ok_or_else(|| FastTrimError::new("trim_failed", "Process disappeared."))?
        .wait()
        .map_err(|error| FastTrimError::new("trim_failed", error.to_string()))?;
    state
        .inner
        .child
        .lock()
        .map_err(|_| FastTrimError::new("trim_failed", "Process lock poisoned."))?
        .take();
    let stderr = stderr_reader.join().unwrap_or_default();
    if state.inner.cancel_requested.swap(false, Ordering::SeqCst) {
        return Err(FastTrimError::new(
            "trim_cancelled",
            "Fast Cut was cancelled safely.",
        ));
    }
    if !status.success() {
        return Err(classify_ffmpeg_error(&stderr));
    }
    Ok(())
}

struct VerifiedTrim {
    duration_seconds: f64,
    video_codec: String,
    audio_codec: Option<String>,
}

fn validate_trim_output(
    path: &Path,
    ffprobe: &Path,
    library_dir: &Path,
    requested_duration: f64,
    audio_removed: bool,
) -> Result<VerifiedTrim, FastTrimError> {
    let size = fs::metadata(path)
        .map_err(|error| FastTrimError::new("output_invalid", error.to_string()))?
        .len();
    if size == 0 {
        return Err(FastTrimError::new(
            "output_empty",
            "The trimmed output is empty.",
        ));
    }
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .env("PATH", path_with_runtime_libraries(library_dir))
        .output()
        .map_err(|error| FastTrimError::new("ffprobe_unavailable", error.to_string()))?;
    if !output.status.success() {
        return Err(FastTrimError::new(
            "output_invalid",
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    let document: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| FastTrimError::new("output_invalid", error.to_string()))?;
    let streams = document["streams"]
        .as_array()
        .ok_or_else(|| FastTrimError::new("output_invalid", "FFprobe found no output streams."))?;
    let video = streams
        .iter()
        .find(|stream| stream["codec_type"].as_str() == Some("video"))
        .ok_or_else(|| FastTrimError::new("output_invalid", "No valid video stream found."))?;
    let audio = streams
        .iter()
        .find(|stream| stream["codec_type"].as_str() == Some("audio"));
    if audio_removed && audio.is_some() {
        return Err(FastTrimError::new(
            "output_invalid",
            "The output unexpectedly contains audio.",
        ));
    }
    let duration = document["format"]["duration"]
        .as_str()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| FastTrimError::new("output_invalid", "The output duration is invalid."))?;
    let tolerance = (requested_duration * 0.5).clamp(1.0, 4.0);
    if (duration - requested_duration).abs() > tolerance {
        return Err(FastTrimError::new(
            "output_duration_invalid",
            format!(
                "The output duration ({duration:.3}s) is not close to the requested duration ({requested_duration:.3}s)."
            ),
        ));
    }
    Ok(VerifiedTrim {
        duration_seconds: duration,
        video_codec: video["codec_name"]
            .as_str()
            .unwrap_or("unknown")
            .to_ascii_uppercase(),
        audio_codec: audio
            .and_then(|stream| stream["codec_name"].as_str())
            .map(str::to_ascii_uppercase),
    })
}

fn detect_previous_keyframe(
    input: &Path,
    ffprobe: &Path,
    library_dir: &Path,
    requested_start: f64,
) -> Option<f64> {
    if requested_start <= 0.0 {
        return Some(0.0);
    }
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-skip_frame",
            "nokey",
            "-show_entries",
            "frame=best_effort_timestamp_time",
            "-of",
            "csv=p=0",
        ])
        .arg(input)
        .env("PATH", path_with_runtime_libraries(library_dir))
        .output()
        .ok()?;
    output.status.success().then_some(())?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().trim_end_matches(',').parse::<f64>().ok())
        .filter(|time| *time <= requested_start + 0.000_5)
        .next_back()
}

fn sanitized_output_name(requested: &str, input: &Path) -> Result<String, FastTrimError> {
    let requested = requested.trim();
    let fallback = format!(
        "{}-trimmed",
        input
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("video")
    );
    let value = if requested.is_empty() {
        fallback.as_str()
    } else {
        requested
    };
    if value.contains(['/', '\\']) || matches!(value, "." | "..") {
        return Err(FastTrimError::new(
            "invalid_output_name",
            "The output file name is invalid.",
        ));
    }
    Ok(Path::new(value)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(&fallback)
        .to_owned())
}

fn destination_directory(input: &Path, requested: Option<&str>) -> Result<PathBuf, FastTrimError> {
    let destination = requested
        .map(PathBuf::from)
        .or_else(|| input.parent().map(Path::to_path_buf))
        .ok_or_else(|| FastTrimError::new("destination_inaccessible", "Missing destination."))?;
    if !destination.is_dir() {
        return Err(FastTrimError::new(
            "destination_inaccessible",
            "The destination folder does not exist or is inaccessible.",
        ));
    }
    Ok(destination)
}

fn temporary_output_path(destination: &Path, stem: &str, extension: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    destination.join(format!(".{stem}.tmp.{nonce}.{extension}"))
}

fn unique_output_path(destination: &Path, stem: &str, extension: &str) -> PathBuf {
    let first = destination.join(format!("{stem}.{extension}"));
    if !first.exists() {
        return first;
    }
    for index in 2..u32::MAX {
        let candidate = destination.join(format!("{stem}-{index}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    destination.join(format!("{stem}-{}.{}", std::process::id(), extension))
}

fn verify_destination_writable(path: &Path) -> Result<(), FastTrimError> {
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(filesystem_write_error)?;
    fs::remove_file(path)
        .map_err(|error| FastTrimError::new("destination_inaccessible", error.to_string()))
}

fn filesystem_write_error(error: std::io::Error) -> FastTrimError {
    let message = error.to_string();
    if error.raw_os_error() == Some(112) || message.to_ascii_lowercase().contains("no space left") {
        FastTrimError::new("disk_full", message)
    } else {
        FastTrimError::new("destination_inaccessible", message)
    }
}

fn classify_ffmpeg_error(stderr: &str) -> FastTrimError {
    let lower = stderr.to_ascii_lowercase();
    let code = if lower.contains("no space left") || lower.contains("not enough space") {
        "disk_full"
    } else if lower.contains("could not find tag")
        || lower.contains("not currently supported in container")
    {
        "container_incompatible"
    } else if lower.contains("permission denied") || lower.contains("access is denied") {
        "destination_inaccessible"
    } else {
        "trim_failed"
    };
    FastTrimError::new(code, stderr.trim())
}

struct TemporaryOutput(Option<PathBuf>);

impl Drop for TemporaryOutput {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video_tools::TARGET_TRIPLE;

    fn tool(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!("{name}-{TARGET_TRIPLE}.exe"))
    }

    fn libraries() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/ffmpeg")
    }

    fn create_source(path: &Path, duration: u64) {
        let output = Command::new(tool("ffmpeg"))
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=30",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=700:sample_rate=44100",
                "-t",
                &duration.to_string(),
                "-c:v",
                "libopenh264",
                "-rc_mode",
                "quality",
                "-qmin",
                "24",
                "-qmax",
                "24",
                "-g",
                "60",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
                "-metadata",
                "title=FileNova Fast Cut",
                "-shortest",
            ])
            .arg(path)
            .env("PATH", path_with_runtime_libraries(&libraries()))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn request(path: &Path, destination: &Path, start: f64, end: f64) -> FastTrimRequest {
        FastTrimRequest {
            path: path.to_string_lossy().into_owned(),
            destination_dir: Some(destination.to_string_lossy().into_owned()),
            preserve_original_format: true,
            output_format: "mp4".into(),
            preserve_metadata: false,
            remove_audio: false,
            output_name: "video-trimmed".into(),
            start_seconds: start,
            end_seconds: end,
        }
    }

    #[test]
    fn compatibility_blocks_invalid_fast_cut_without_reencoding_fallback() {
        assert!(video_copy_compatible("mp4", "H264"));
        assert!(audio_copy_compatible("mp4", Some("AAC")));
        assert!(!video_copy_compatible("webm", "H264"));
        assert!(!audio_copy_compatible("webm", Some("AAC")));
        let req = request(Path::new("C:\\in.mp4"), Path::new("C:\\"), 1.0, 2.0);
        let args = fast_cut_args(
            &req,
            Path::new("C:\\in.mp4"),
            Path::new("C:\\out.mp4"),
            1.0,
            "mp4",
        )
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
        assert!(args.contains("-ss 1.000000 -i"));
        assert!(args.contains("-t 1.000000"));
        assert!(args.contains("-c copy"));
        for forbidden in ["libopenh264", "libsvtav1", "libx264", "-crf", "-preset"] {
            assert!(!args.contains(forbidden));
        }
    }

    #[test]
    fn really_fast_cuts_mp4_at_zero_middle_and_near_end_with_unique_names() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("outputs");
        fs::create_dir(&destination).unwrap();
        let source = temp.path().join("source.mp4");
        create_source(&source, 10);
        let original = fs::read(&source).unwrap();
        let state = TrimVideoState::default();

        let zero = fast_trim_with_tools(
            request(&source, &destination, 0.0, 3.0),
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &state,
            |_| {},
        )
        .unwrap();
        let middle = fast_trim_with_tools(
            request(&source, &destination, 3.2, 6.4),
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &state,
            |_| {},
        )
        .unwrap();
        let near_end = fast_trim_with_tools(
            request(&source, &destination, 8.0, 9.8),
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &state,
            |_| {},
        )
        .unwrap();
        assert_eq!(zero.output_name, "video-trimmed.mp4");
        assert_eq!(middle.output_name, "video-trimmed-2.mp4");
        assert_eq!(near_end.output_name, "video-trimmed-3.mp4");
        assert_eq!(zero.final_video_codec, "H264");
        assert_eq!(zero.final_audio_codec.as_deref(), Some("AAC"));
        assert!(middle.detected_actual_start_seconds <= 3.2);
        assert!(middle.keyframe_offset_seconds <= 0.0);
        assert!((near_end.final_duration_seconds - 1.8).abs() <= 1.0);
        assert_eq!(fs::read(&source).unwrap(), original);
    }

    #[test]
    fn really_fast_cuts_mkv_removes_audio_and_preserves_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("outputs");
        fs::create_dir(&destination).unwrap();
        let source = temp.path().join("movie.mkv");
        create_source(&source, 8);
        let original = fs::read(&source).unwrap();
        let state = TrimVideoState::default();

        let mkv = fast_trim_with_tools(
            request(&source, &destination, 1.0, 5.0),
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &state,
            |_| {},
        )
        .unwrap();
        assert_eq!(mkv.final_format, "MKV");
        assert_eq!(mkv.final_audio_codec.as_deref(), Some("AAC"));

        let mut silent = request(&source, &destination, 2.0, 6.0);
        silent.remove_audio = true;
        silent.preserve_metadata = true;
        silent.output_name = "custom-name.mkv".into();
        let silent = fast_trim_with_tools(
            silent,
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &state,
            |_| {},
        )
        .unwrap();
        assert_eq!(silent.output_name, "custom-name.mkv");
        assert_eq!(silent.final_audio_codec, None);
        let metadata = Command::new(tool("ffprobe"))
            .args([
                "-v",
                "error",
                "-show_entries",
                "format_tags=title",
                "-of",
                "default=nw=1",
            ])
            .arg(&silent.output_path)
            .env("PATH", path_with_runtime_libraries(&libraries()))
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&metadata.stdout).contains("FileNova Fast Cut"));
        assert_eq!(fs::read(&source).unwrap(), original);
    }

    #[test]
    fn incompatible_webm_is_blocked_before_any_output_is_created() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("outputs");
        fs::create_dir(&destination).unwrap();
        let source = temp.path().join("source.mp4");
        create_source(&source, 3);
        let mut conversion = request(&source, &destination, 0.0, 2.0);
        conversion.preserve_original_format = false;
        conversion.output_format = "webm".into();
        let error = fast_trim_with_tools(
            conversion,
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &TrimVideoState::default(),
            |_| {},
        )
        .unwrap_err();
        assert_eq!(error.code, "fast_cut_video_incompatible");
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    }

    #[test]
    fn cancellation_kills_only_active_ffmpeg_and_temporary_guard_cleans_output() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.mp4");
        let output = temp.path().join(".cancelled.tmp.mp4");
        create_source(&source, 8);
        let original = fs::read(&source).unwrap();
        let state = TrimVideoState::default();
        let worker_state = state.clone();
        let worker_source = source.clone();
        let worker_output = output.clone();
        let worker = thread::spawn(move || {
            let _cleanup = TemporaryOutput(Some(worker_output.clone()));
            let args = vec![
                "-hide_banner".into(),
                "-nostdin".into(),
                "-v".into(),
                "warning".into(),
                "-re".into(),
                "-i".into(),
                worker_source.as_os_str().to_owned(),
                "-t".into(),
                "7".into(),
                "-map".into(),
                "0:v:0".into(),
                "-map".into(),
                "0:a:0?".into(),
                "-c".into(),
                "copy".into(),
                "-progress".into(),
                "pipe:1".into(),
                "-nostats".into(),
                worker_output.as_os_str().to_owned(),
            ];
            run_ffmpeg(
                &tool("ffmpeg"),
                &libraries(),
                &worker_state,
                &args,
                &worker_source.to_string_lossy(),
                "source.mp4",
                7.0,
                &mut |_| {},
            )
        });
        let mut killed = false;
        for _ in 0..1000 {
            if let Ok(mut active) = state.inner.child.lock() {
                if let Some(child) = active.as_mut() {
                    state.inner.cancel_requested.store(true, Ordering::SeqCst);
                    child.kill().unwrap();
                    killed = true;
                    break;
                }
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(killed);
        assert_eq!(worker.join().unwrap().unwrap_err().code, "trim_cancelled");
        assert!(!output.exists());
        assert_eq!(fs::read(&source).unwrap(), original);
    }
}
