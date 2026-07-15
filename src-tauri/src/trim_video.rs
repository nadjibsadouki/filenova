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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreciseTrimRequest {
    path: String,
    destination_dir: Option<String>,
    preserve_original_format: bool,
    output_format: String,
    preserve_metadata: bool,
    remove_audio: bool,
    output_name: String,
    start_seconds: f64,
    end_seconds: f64,
    quality: u8,
    video_codec: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreciseTrimResult {
    method: String,
    requested_start_seconds: f64,
    requested_end_seconds: f64,
    requested_duration_seconds: f64,
    final_duration_seconds: f64,
    duration_difference_seconds: f64,
    final_format: String,
    final_video_codec: String,
    final_audio_codec: Option<String>,
    quality: u8,
    width: u64,
    height: u64,
    frames_per_second: f64,
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

#[tauri::command]
pub async fn precise_trim_video(
    app: AppHandle,
    window: Window,
    state: State<'_, TrimVideoState>,
    request: PreciseTrimRequest,
) -> Result<PreciseTrimResult, FastTrimError> {
    let ffmpeg = find_sidecar(&app, "ffmpeg")
        .map_err(|error| FastTrimError::new("ffmpeg_unavailable", error))?;
    let ffprobe = find_sidecar(&app, "ffprobe")
        .map_err(|error| FastTrimError::new("ffprobe_unavailable", error))?;
    let library_dir = runtime_library_dir(&app);
    let trim_state = state.inner().clone();
    let progress_window = window.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        precise_trim_with_tools(
            request,
            &ffmpeg,
            &ffprobe,
            &library_dir,
            &trim_state,
            |progress| {
                let _ = progress_window.emit("precise-trim-progress", progress);
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
pub fn cancel_precise_trim(state: State<'_, TrimVideoState>) -> Result<bool, FastTrimError> {
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

fn precise_trim_with_tools<F>(
    request: PreciseTrimRequest,
    ffmpeg: &Path,
    ffprobe: &Path,
    library_dir: &Path,
    state: &TrimVideoState,
    mut report: F,
) -> Result<PreciseTrimResult, FastTrimError>
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
        .ok_or_else(|| FastTrimError::new("format_unavailable", "Missing source container."))?;
    let container = if request.preserve_original_format {
        source_extension
    } else {
        request.output_format.to_ascii_lowercase()
    };
    if !matches!(container.as_str(), "mp4" | "mov" | "mkv" | "webm") {
        return Err(FastTrimError::new(
            "format_unavailable",
            "Precise Cut supports MP4, MOV, MKV and WebM outputs.",
        ));
    }
    if !(1..=100).contains(&request.quality) {
        return Err(FastTrimError::new(
            "invalid_quality",
            "Precise Cut quality must be between 1 and 100.",
        ));
    }
    let (video_encoder, audio_encoder) = precise_encoders(&container, &request.video_codec)?;
    ensure_encoders_available(
        ffmpeg,
        library_dir,
        video_encoder,
        (!request.remove_audio && input.audio_codec.is_some()).then_some(audio_encoder),
    )?;
    let destination = destination_directory(&input_path, request.destination_dir.as_deref())?;
    let base_name = sanitized_precise_output_name(&request.output_name, &input_path)?;
    let temporary = temporary_output_path(&destination, &base_name, &container);
    verify_destination_writable(&temporary)?;
    let mut cleanup = TemporaryOutput(Some(temporary.clone()));
    let requested_duration = request.end_seconds - request.start_seconds;
    let args = precise_cut_args(
        &request,
        &input_path,
        &temporary,
        requested_duration,
        &container,
        video_encoder,
        audio_encoder,
        input.audio_codec.is_some(),
        input.frames_per_second,
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
    let verified = validate_precise_output(
        &temporary,
        ffprobe,
        library_dir,
        requested_duration,
        request.remove_audio || input.audio_codec.is_none(),
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
    Ok(PreciseTrimResult {
        method: "Precise Cut".into(),
        requested_start_seconds: request.start_seconds,
        requested_end_seconds: request.end_seconds,
        requested_duration_seconds: requested_duration,
        final_duration_seconds: verified.duration_seconds,
        duration_difference_seconds: verified.duration_seconds - requested_duration,
        final_format: if container == "webm" {
            "WebM".into()
        } else {
            container.to_ascii_uppercase()
        },
        final_video_codec: verified.video_codec,
        final_audio_codec: verified.audio_codec,
        quality: request.quality,
        width: verified.width,
        height: verified.height,
        frames_per_second: verified.frames_per_second,
        original_size: input.size,
        final_size,
        processing_duration_seconds: started_at.elapsed().as_secs_f64(),
        output_name: final_output
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("video-precise-trim")
            .to_owned(),
        output_path: final_output.to_string_lossy().into_owned(),
    })
}

fn precise_encoders<'a>(
    container: &str,
    requested: &'a str,
) -> Result<(&'a str, &'a str), FastTrimError> {
    match container {
        "mp4" | "mov" => Ok(("libopenh264", "aac")),
        "webm" => Ok(("libsvtav1", "libopus")),
        "mkv" => match requested.to_ascii_lowercase().as_str() {
            "auto" | "openh264" => Ok(("libopenh264", "aac")),
            "svtav1" | "svt-av1" => Ok(("libsvtav1", "libopus")),
            _ => Err(FastTrimError::new(
                "encoder_unavailable",
                "The selected MKV video encoder is unavailable.",
            )),
        },
        _ => Err(FastTrimError::new(
            "format_unavailable",
            "Unsupported Precise Cut output format.",
        )),
    }
}

fn ensure_encoders_available(
    ffmpeg: &Path,
    library_dir: &Path,
    video_encoder: &str,
    audio_encoder: Option<&str>,
) -> Result<(), FastTrimError> {
    let output = Command::new(ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .env("PATH", path_with_runtime_libraries(library_dir))
        .output()
        .map_err(|error| FastTrimError::new("ffmpeg_unavailable", error.to_string()))?;
    if !output.status.success() {
        return Err(FastTrimError::new(
            "encoder_unavailable",
            String::from_utf8_lossy(&output.stderr),
        ));
    }
    let encoders = String::from_utf8_lossy(&output.stdout);
    if !encoders.contains(video_encoder)
        || audio_encoder.is_some_and(|codec| !encoders.contains(codec))
    {
        return Err(FastTrimError::new(
            "encoder_unavailable",
            "A required Precise Cut encoder is not present in the bundled FFmpeg build.",
        ));
    }
    Ok(())
}

fn openh264_quality_value(quality: u8) -> u8 {
    interpolate_quality(quality, &[(1, 35), (50, 26), (70, 23), (85, 20), (100, 17)])
}

fn svt_av1_quality_value(quality: u8) -> u8 {
    interpolate_quality(quality, &[(1, 50), (50, 35), (75, 28), (100, 18)])
}

fn interpolate_quality(quality: u8, points: &[(u8, u8)]) -> u8 {
    let quality = quality.clamp(1, 100);
    for pair in points.windows(2) {
        let ((left_quality, left_value), (right_quality, right_value)) = (pair[0], pair[1]);
        if quality <= right_quality {
            let position = (quality - left_quality) as f64 / (right_quality - left_quality) as f64;
            return (left_value as f64 + (right_value as f64 - left_value as f64) * position).round()
                as u8;
        }
    }
    points.last().map(|point| point.1).unwrap_or(23)
}

#[allow(clippy::too_many_arguments)]
fn precise_cut_args(
    request: &PreciseTrimRequest,
    input: &Path,
    output: &Path,
    duration: f64,
    container: &str,
    video_encoder: &str,
    audio_encoder: &str,
    has_audio: bool,
    frames_per_second: f64,
) -> Vec<OsString> {
    let mut args = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-v".into(),
        "warning".into(),
        "-n".into(),
        "-i".into(),
        input.as_os_str().to_owned(),
        "-ss".into(),
        format!("{:.6}", request.start_seconds).into(),
        "-t".into(),
        format!("{duration:.6}").into(),
        "-map".into(),
        "0:v:0".into(),
    ];
    if request.remove_audio || !has_audio {
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
        "-vf".into(),
        "setpts=PTS-STARTPTS".into(),
        "-r".into(),
        format!("{frames_per_second:.6}").into(),
        "-c:v".into(),
        video_encoder.into(),
    ]);
    if video_encoder == "libopenh264" {
        let value = openh264_quality_value(request.quality).to_string();
        args.extend([
            "-rc_mode".into(),
            "quality".into(),
            "-qmin".into(),
            value.clone().into(),
            "-qmax".into(),
            value.into(),
            "-pix_fmt".into(),
            "yuv420p".into(),
            "-profile:v".into(),
            "main".into(),
        ]);
    } else {
        args.extend([
            "-crf".into(),
            svt_av1_quality_value(request.quality).to_string().into(),
            "-preset".into(),
            "8".into(),
            "-pix_fmt".into(),
            "yuv420p".into(),
        ]);
    }
    if !request.remove_audio && has_audio {
        args.extend([
            "-af".into(),
            "asetpts=PTS-STARTPTS".into(),
            "-c:a".into(),
            audio_encoder.into(),
            "-b:a".into(),
            if audio_encoder == "libopus" {
                "128k".into()
            } else {
                "160k".into()
            },
        ]);
    }
    args.extend(["-avoid_negative_ts".into(), "make_zero".into()]);
    if container == "mp4" {
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

struct VerifiedPreciseTrim {
    duration_seconds: f64,
    video_codec: String,
    audio_codec: Option<String>,
    width: u64,
    height: u64,
    frames_per_second: f64,
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

fn validate_precise_output(
    path: &Path,
    ffprobe: &Path,
    library_dir: &Path,
    requested_duration: f64,
    audio_removed: bool,
) -> Result<VerifiedPreciseTrim, FastTrimError> {
    let size = fs::metadata(path)
        .map_err(|error| FastTrimError::new("output_invalid", error.to_string()))?
        .len();
    if size == 0 {
        return Err(FastTrimError::new(
            "output_empty",
            "The precise trim output is empty.",
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
    let fps = parse_frame_rate(video["avg_frame_rate"].as_str().unwrap_or("0/1"));
    // Two output frames or 120 ms (whichever is greater) covers encoder/container
    // rounding while still detecting cuts that are visibly imprecise.
    let duration_tolerance = (if fps > 0.0 { 2.0 / fps } else { 0.0 }).max(0.120);
    if (duration - requested_duration).abs() > duration_tolerance {
        return Err(FastTrimError::new(
            "output_duration_invalid",
            format!("Precise Cut differs by more than the documented {duration_tolerance:.3}s tolerance."),
        ));
    }
    if let Some(audio) = audio {
        let video_duration = stream_duration(video).unwrap_or(duration);
        let audio_duration = stream_duration(audio).unwrap_or(duration);
        let sync_tolerance = duration_tolerance.max(0.200);
        if (video_duration - audio_duration).abs() > sync_tolerance {
            return Err(FastTrimError::new(
                "output_desynchronized",
                format!("Audio and video differ by more than {sync_tolerance:.3}s."),
            ));
        }
    }
    Ok(VerifiedPreciseTrim {
        duration_seconds: duration,
        video_codec: video["codec_name"]
            .as_str()
            .unwrap_or("unknown")
            .to_ascii_uppercase(),
        audio_codec: audio
            .and_then(|stream| stream["codec_name"].as_str())
            .map(str::to_ascii_uppercase),
        width: video["width"].as_u64().unwrap_or(0),
        height: video["height"].as_u64().unwrap_or(0),
        frames_per_second: fps,
    })
}

fn stream_duration(stream: &serde_json::Value) -> Option<f64> {
    stream["duration"]
        .as_str()?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

fn parse_frame_rate(value: &str) -> f64 {
    let (numerator, denominator) = value.split_once('/').unwrap_or((value, "1"));
    let numerator = numerator.parse::<f64>().unwrap_or(0.0);
    let denominator = denominator.parse::<f64>().unwrap_or(1.0);
    if denominator > 0.0 {
        numerator / denominator
    } else {
        0.0
    }
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

fn sanitized_precise_output_name(requested: &str, input: &Path) -> Result<String, FastTrimError> {
    let requested = requested.trim();
    let fallback = format!(
        "{}-precise-trim",
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
    } else if lower.contains("unknown encoder")
        || lower.contains("error setting encoder")
        || lower.contains("error while opening encoder")
    {
        "encoder_unavailable"
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

    fn precise_request(
        path: &Path,
        destination: &Path,
        start: f64,
        end: f64,
    ) -> PreciseTrimRequest {
        PreciseTrimRequest {
            path: path.to_string_lossy().into_owned(),
            destination_dir: Some(destination.to_string_lossy().into_owned()),
            preserve_original_format: true,
            output_format: "mp4".into(),
            preserve_metadata: false,
            remove_audio: false,
            output_name: "video-precise-trim".into(),
            start_seconds: start,
            end_seconds: end,
            quality: 75,
            video_codec: "auto".into(),
        }
    }

    #[test]
    fn precise_args_reencode_with_validated_quality_options_and_never_copy_video() {
        let mut req = precise_request(Path::new("C:\\in.mp4"), Path::new("C:\\"), 1.137, 3.789);
        for quality in [1, 75, 100] {
            req.quality = quality;
            let args = precise_cut_args(
                &req,
                Path::new("C:\\in.mp4"),
                Path::new("C:\\out.mp4"),
                2.652,
                "mp4",
                "libopenh264",
                "aac",
                true,
                30.0,
            )
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
            assert!(args.contains("-i C:\\in.mp4 -ss 1.137000 -t 2.652000"));
            assert!(args.contains("-c:v libopenh264 -rc_mode quality"));
            assert!(args.contains("-pix_fmt yuv420p -profile:v main"));
            assert!(args.contains("-c:a aac -b:a 160k"));
            assert!(args.contains("-movflags +faststart"));
            for forbidden in ["-c copy", "libx264", "preset medium", "-pass", "passlog"] {
                assert!(!args.contains(forbidden));
            }
        }
        assert_eq!(openh264_quality_value(1), 35);
        assert_eq!(openh264_quality_value(70), 23);
        assert_eq!(openh264_quality_value(100), 17);
        assert_eq!(svt_av1_quality_value(1), 50);
        assert_eq!(svt_av1_quality_value(75), 28);
        assert_eq!(svt_av1_quality_value(100), 18);
    }

    #[test]
    fn really_precise_trims_mp4_at_milliseconds_between_keyframes_and_preserves_original() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("outputs");
        fs::create_dir(&destination).unwrap();
        let source = temp.path().join("source.mp4");
        create_source(&source, 6);
        let original = fs::read(&source).unwrap();
        let result = precise_trim_with_tools(
            precise_request(&source, &destination, 1.137, 3.789),
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &TrimVideoState::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.final_video_codec, "H264");
        assert_eq!(result.final_audio_codec.as_deref(), Some("AAC"));
        assert!((result.final_duration_seconds - 2.652).abs() <= 0.120);
        assert!(result.duration_difference_seconds.abs() <= 0.120);
        assert_eq!((result.width, result.height), (320, 180));
        assert!((result.frames_per_second - 30.0).abs() < 0.01);
        assert_eq!(fs::read(&source).unwrap(), original);
    }

    #[test]
    fn really_precise_converts_mov_to_mp4_and_mp4_to_webm() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("outputs");
        fs::create_dir(&destination).unwrap();
        let mov = temp.path().join("source.mov");
        create_source(&mov, 4);
        let mut mp4_req = precise_request(&mov, &destination, 0.333, 2.666);
        mp4_req.preserve_original_format = false;
        mp4_req.output_format = "mp4".into();
        let mp4 = precise_trim_with_tools(
            mp4_req,
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &TrimVideoState::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(mp4.final_format, "MP4");
        assert_eq!(mp4.final_video_codec, "H264");

        let mut webm_req = precise_request(Path::new(&mp4.output_path), &destination, 0.111, 1.777);
        webm_req.preserve_original_format = false;
        webm_req.output_format = "webm".into();
        webm_req.quality = 25;
        let webm = precise_trim_with_tools(
            webm_req,
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &TrimVideoState::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(webm.final_format, "WebM");
        assert_eq!(webm.final_video_codec, "AV1");
        assert_eq!(webm.final_audio_codec.as_deref(), Some("OPUS"));
    }

    #[test]
    fn precise_mkv_supports_openh264_metadata_no_audio_and_unique_names() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("outputs");
        fs::create_dir(&destination).unwrap();
        let source = temp.path().join("source.mkv");
        create_source(&source, 4);
        let original = fs::read(&source).unwrap();
        for index in 0..2 {
            let mut req = precise_request(&source, &destination, 0.567, 2.901);
            req.remove_audio = true;
            req.preserve_metadata = true;
            req.quality = if index == 0 { 20 } else { 95 };
            req.video_codec = "openh264".into();
            let result = precise_trim_with_tools(
                req,
                &tool("ffmpeg"),
                &tool("ffprobe"),
                &libraries(),
                &TrimVideoState::default(),
                |_| {},
            )
            .unwrap();
            assert_eq!(result.final_video_codec, "H264");
            assert_eq!(result.final_audio_codec, None);
            assert_eq!(
                result.output_name,
                if index == 0 {
                    "video-precise-trim.mkv"
                } else {
                    "video-precise-trim-2.mkv"
                }
            );
            let metadata = Command::new(tool("ffprobe"))
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "format_tags=title",
                    "-of",
                    "default=nw=1",
                ])
                .arg(&result.output_path)
                .env("PATH", path_with_runtime_libraries(&libraries()))
                .output()
                .unwrap();
            assert!(String::from_utf8_lossy(&metadata.stdout).contains("FileNova Fast Cut"));
        }
        assert_eq!(fs::read(&source).unwrap(), original);
    }

    #[test]
    fn precise_is_frame_accurate_between_keyframes_and_fast_cut_remains_available() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("outputs");
        fs::create_dir(&destination).unwrap();
        let source = temp.path().join("source.mp4");
        create_source(&source, 6);
        let fast = fast_trim_with_tools(
            request(&source, &destination, 1.137, 3.789),
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &TrimVideoState::default(),
            |_| {},
        )
        .unwrap();
        let precise = precise_trim_with_tools(
            precise_request(&source, &destination, 1.137, 3.789),
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &TrimVideoState::default(),
            |_| {},
        )
        .unwrap();
        assert!(fast.keyframe_offset_seconds.abs() > 0.5);
        assert!(precise.duration_difference_seconds.abs() <= 0.120);
        assert!(fast.processing_duration_seconds > 0.0);
        assert!(precise.processing_duration_seconds > 0.0);
        eprintln!(
            "Fast Cut: keyframe offset={:.3}s, output duration={:.3}s, processing={:.3}s; Precise Cut: duration difference={:.3}s, output duration={:.3}s, processing={:.3}s",
            fast.keyframe_offset_seconds,
            fast.final_duration_seconds,
            fast.processing_duration_seconds,
            precise.duration_difference_seconds,
            precise.final_duration_seconds,
            precise.processing_duration_seconds,
        );
    }

    #[test]
    fn cancelling_precise_encoding_removes_temporary_output_and_preserves_original() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.mp4");
        let output = temp.path().join(".precise-cancelled.tmp.mp4");
        create_source(&source, 8);
        let original = fs::read(&source).unwrap();
        let state = TrimVideoState::default();
        let worker_state = state.clone();
        let worker_source = source.clone();
        let worker_output = output.clone();
        let worker = thread::spawn(move || {
            let _cleanup = TemporaryOutput(Some(worker_output.clone()));
            let request = precise_request(
                &worker_source,
                worker_source.parent().unwrap(),
                0.333,
                7.333,
            );
            let mut args = precise_cut_args(
                &request,
                &worker_source,
                &worker_output,
                7.0,
                "mp4",
                "libopenh264",
                "aac",
                true,
                30.0,
            );
            let input_position = args.iter().position(|value| value == "-i").unwrap();
            args.insert(input_position, "-re".into());
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
