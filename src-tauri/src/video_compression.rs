use std::{
    collections::{HashMap, HashSet},
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

use crate::video_target_size::{
    calculate_target_bitrates, choose_target_resolution, corrected_video_bitrate_kbps,
    minimum_usable_video_bitrate_kbps, MIN_OPENH264_BITS_PER_PIXEL_PER_FRAME,
    TARGET_SIZE_SAFETY_MARGIN,
};
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
    pending_results: Mutex<HashMap<PathBuf, PendingVideoResult>>,
}

#[derive(Debug, Clone)]
struct PendingVideoResult {
    destination: PathBuf,
    base_name: String,
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
    quality: Option<u8>,
    estimated_size: Option<u64>,
    target_size_bytes: Option<u64>,
    allow_automatic_resolution_reduction: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoCompressionProgress {
    path: String,
    name: String,
    processed_seconds: f64,
    duration_seconds: f64,
    percent: f64,
    phase: String,
    attempt: u8,
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
    quality: Option<u8>,
    encoding_setting: String,
    estimated_size: Option<u64>,
    estimate_difference: Option<i64>,
    target_size: Option<u64>,
    initial_video_bitrate_kbps: Option<f64>,
    corrected_video_bitrate_kbps: Option<f64>,
    attempts: u8,
    target_exceeded: bool,
    pending_confirmation: bool,
    original_width: u64,
    original_height: u64,
    resolution_reduced_automatically: bool,
    resolution_reduction_reason: Option<String>,
    #[serde(skip_serializing)]
    pending_final_base: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeptVideoResult {
    output_path: String,
    output_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebmEncoder {
    SvtAv1,
    Vp9,
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
    if result.pending_confirmation {
        let base_name = result.pending_final_base.clone().ok_or_else(|| {
            VideoCompressionError::new("output_invalid", "Pending result metadata is missing.")
        })?;
        let destination = output_path.parent().map(Path::to_path_buf).ok_or_else(|| {
            VideoCompressionError::new("output_invalid", "Pending result destination is missing.")
        })?;
        state
            .inner
            .pending_results
            .lock()
            .map_err(|_| {
                VideoCompressionError::new("encoding_failed", "Pending result lock is poisoned.")
            })?
            .insert(
                output_path,
                PendingVideoResult {
                    destination,
                    base_name,
                },
            );
    } else {
        state
            .inner
            .completed_results
            .lock()
            .map_err(|_| VideoCompressionError::new("encoding_failed", "Result lock is poisoned."))?
            .insert(output_path);
    }

    Ok(result)
}

#[tauri::command]
pub fn keep_video_target_result(
    app: AppHandle,
    state: State<'_, VideoCompressionState>,
    path: String,
) -> Result<KeptVideoResult, VideoCompressionError> {
    let temporary = PathBuf::from(path);
    let pending = state
        .inner
        .pending_results
        .lock()
        .map_err(|_| {
            VideoCompressionError::new("encoding_failed", "Pending result lock is poisoned.")
        })?
        .get(&temporary)
        .cloned()
        .ok_or_else(|| {
            VideoCompressionError::new(
                "output_invalid",
                "This result is not awaiting confirmation.",
            )
        })?;
    let final_output = unique_output_path(&pending.destination, &pending.base_name, "mp4");
    fs::rename(&temporary, &final_output).map_err(|error| {
        VideoCompressionError::new("destination_inaccessible", error.to_string())
    })?;
    state
        .inner
        .pending_results
        .lock()
        .map_err(|_| {
            VideoCompressionError::new("encoding_failed", "Pending result lock is poisoned.")
        })?
        .remove(&temporary);
    app.asset_protocol_scope()
        .allow_file(&final_output)
        .map_err(|error| VideoCompressionError::new("output_invalid", error.to_string()))?;
    state
        .inner
        .completed_results
        .lock()
        .map_err(|_| VideoCompressionError::new("encoding_failed", "Result lock is poisoned."))?
        .insert(final_output.clone());
    Ok(KeptVideoResult {
        output_name: final_output
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("video-target.mp4")
            .to_owned(),
        output_path: final_output.to_string_lossy().into_owned(),
    })
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
    if state
        .inner
        .pending_results
        .lock()
        .map_err(|_| {
            VideoCompressionError::new("delete_failed", "Pending result lock is poisoned.")
        })?
        .remove(&path)
        .is_some()
    {
        return fs::remove_file(&path)
            .map_err(|error| VideoCompressionError::new("delete_failed", error.to_string()));
    }
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
    if request.mode == "targetFileSize" {
        return compress_target_mp4_with_tools(
            request,
            ffmpeg,
            ffprobe,
            library_dir,
            state,
            report,
        );
    }
    if request.mode != "recommended" && request.mode != "targetQuality" {
        return Err(VideoCompressionError::new(
            "mode_not_implemented",
            "Target File Size is not implemented.",
        ));
    }
    let selected_quality = if request.mode == "targetQuality" {
        Some(request.quality.unwrap_or(70).clamp(1, 100))
    } else {
        None
    };
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

    let webm_encoder = if request.output_format == "webm" {
        Some(detect_webm_encoder(ffmpeg, library_dir)?)
    } else {
        None
    };
    let (args, encoding_setting) = ffmpeg_args(
        &request,
        selected_quality,
        webm_encoder,
        &input_path,
        &temporary,
    );
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
                    phase: "encoding".into(),
                    attempt: 1,
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
    let estimate_difference = request
        .estimated_size
        .map(|estimate| compressed_size as i64 - estimate as i64);
    report(VideoCompressionProgress {
        path: request.path,
        name: input_name,
        processed_seconds: input.duration_seconds,
        duration_seconds: input.duration_seconds,
        percent: 100.0,
        phase: "encoding".into(),
        attempt: 1,
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
        quality: selected_quality,
        encoding_setting,
        estimated_size: request.estimated_size,
        estimate_difference,
        target_size: None,
        initial_video_bitrate_kbps: None,
        corrected_video_bitrate_kbps: None,
        attempts: 1,
        target_exceeded: false,
        pending_confirmation: false,
        original_width: input.width,
        original_height: input.height,
        resolution_reduced_automatically: false,
        resolution_reduction_reason: None,
        pending_final_base: None,
    })
}

struct TemporaryOutputs(Vec<PathBuf>);

impl TemporaryOutputs {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn track(&mut self, path: PathBuf) {
        self.0.push(path);
    }

    fn keep(&mut self, path: &Path) {
        self.0.retain(|candidate| candidate != path);
    }
}

impl Drop for TemporaryOutputs {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

fn compress_target_mp4_with_tools<F>(
    request: VideoCompressionRequest,
    ffmpeg: &Path,
    ffprobe: &Path,
    library_dir: &Path,
    state: &VideoCompressionState,
    report: F,
) -> Result<VideoCompressionResult, VideoCompressionError>
where
    F: FnMut(VideoCompressionProgress),
{
    compress_target_mp4_with_measure(
        request,
        ffmpeg,
        ffprobe,
        library_dir,
        state,
        report,
        |_, path| {
            fs::metadata(path)
                .map(|metadata| metadata.len())
                .map_err(|error| VideoCompressionError::new("output_invalid", error.to_string()))
        },
    )
}

fn compress_target_mp4_with_measure<F, M>(
    request: VideoCompressionRequest,
    ffmpeg: &Path,
    ffprobe: &Path,
    library_dir: &Path,
    state: &VideoCompressionState,
    mut report: F,
    mut measure: M,
) -> Result<VideoCompressionResult, VideoCompressionError>
where
    F: FnMut(VideoCompressionProgress),
    M: FnMut(u8, &Path) -> Result<u64, VideoCompressionError>,
{
    if request.output_format != "mp4" {
        return Err(VideoCompressionError::new(
            "target_webm_not_implemented",
            "Target File Size currently supports MP4 only.",
        ));
    }
    let target_size = request
        .target_size_bytes
        .filter(|size| *size > 0)
        .ok_or_else(|| {
            VideoCompressionError::new("invalid_target_size", "A positive target size is required.")
        })?;
    let input_path = PathBuf::from(&request.path);
    let input = inspect_video_with_tools(&input_path, ffprobe, ffmpeg, library_dir)
        .map_err(|error| VideoCompressionError::new(error.code, error.message))?;
    if input.duration_seconds <= 0.0 {
        return Err(VideoCompressionError::new(
            "invalid_video",
            "The video duration is invalid.",
        ));
    }
    let audio_kbps = if request.remove_audio || input.audio_codec.is_none() {
        0.0
    } else {
        160.0
    };
    let initial = calculate_target_bitrates(
        target_size as f64,
        input.duration_seconds,
        audio_kbps,
        TARGET_SIZE_SAFETY_MARGIN,
    )
    .ok_or_else(|| {
        VideoCompressionError::new(
            "invalid_target_size",
            "The target bitrate could not be calculated.",
        )
    })?;
    let automatic_reduction = request
        .allow_automatic_resolution_reduction
        .unwrap_or(false);
    let resolution = choose_target_resolution(
        input.width,
        input.height,
        input.frames_per_second,
        initial.video_kbps,
        automatic_reduction,
    )
    .ok_or_else(|| {
        VideoCompressionError::new(
            "invalid_video",
            "The output resolution could not be calculated.",
        )
    })?;
    let resolution_filter = (resolution.width != input.width || resolution.height != input.height)
        .then_some((resolution.width, resolution.height));
    let minimum = if automatic_reduction {
        MIN_OPENH264_BITS_PER_PIXEL_PER_FRAME
            * resolution.width as f64
            * resolution.height as f64
            * input.frames_per_second
            / 1000.0
    } else {
        minimum_usable_video_bitrate_kbps(input.width, input.height)
    };
    if initial.video_kbps < minimum {
        return Err(VideoCompressionError::new(
            "target_bitrate_too_low",
            format!("The calculated video bitrate ({:.0} kbps) is below the usable minimum ({minimum:.0} kbps).", initial.video_kbps),
        ));
    }

    let destination = destination_directory(&input_path, request.destination_dir.as_deref())?;
    let stem = input_path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("video");
    let target_label = target_size_label(target_size);
    let base_name = format!("{stem}-target-{target_label}");
    let mut cleanup = TemporaryOutputs::new();
    let first_temporary =
        temporary_output_path(&destination, &format!("{base_name}-attempt-1"), "mp4");
    verify_destination_writable(&first_temporary)?;
    cleanup.track(first_temporary.clone());
    state.inner.cancel_requested.store(false, Ordering::SeqCst);

    report(VideoCompressionProgress {
        path: request.path.clone(),
        name: input.name.clone(),
        processed_seconds: 0.0,
        duration_seconds: input.duration_seconds,
        percent: 0.0,
        phase: "encodingAttempt1".into(),
        attempt: 1,
    });
    let first_args = target_mp4_ffmpeg_args(
        &request,
        &input_path,
        &first_temporary,
        initial.video_kbps,
        resolution_filter,
    );
    run_target_attempt(
        ffmpeg,
        library_dir,
        state,
        &first_args,
        &request.path,
        &input.name,
        input.duration_seconds,
        "encodingAttempt1",
        1,
        &mut report,
    )?;
    let mut temporary = first_temporary;
    let mut verified = validate_target_output(&temporary, ffprobe, ffmpeg, library_dir)?;
    let mut actual_size = measure(1, &temporary)?;
    let mut corrected_bitrate = None;
    let mut attempts = 1;

    if target_exceeded_by_five_percent(actual_size, target_size) {
        let corrected = corrected_video_bitrate_kbps(initial.video_kbps, target_size, actual_size)
            .ok_or_else(|| {
                VideoCompressionError::new(
                    "invalid_target_size",
                    "The corrected bitrate could not be calculated.",
                )
            })?;
        if corrected < minimum {
            return Err(VideoCompressionError::new(
                "target_bitrate_too_low",
                format!("The corrected video bitrate ({corrected:.0} kbps) is below the usable minimum ({minimum:.0} kbps)."),
            ));
        }
        corrected_bitrate = Some(corrected);
        attempts = 2;
        fs::remove_file(&temporary)
            .map_err(|error| VideoCompressionError::new("output_invalid", error.to_string()))?;
        let second_temporary =
            temporary_output_path(&destination, &format!("{base_name}-attempt-2"), "mp4");
        verify_destination_writable(&second_temporary)?;
        cleanup.track(second_temporary.clone());
        temporary = second_temporary;
        report(VideoCompressionProgress {
            path: request.path.clone(),
            name: input.name.clone(),
            processed_seconds: 0.0,
            duration_seconds: input.duration_seconds,
            percent: 0.0,
            phase: "adjustingBitrate".into(),
            attempt: 2,
        });
        let second_args = target_mp4_ffmpeg_args(
            &request,
            &input_path,
            &temporary,
            corrected,
            resolution_filter,
        );
        run_target_attempt(
            ffmpeg,
            library_dir,
            state,
            &second_args,
            &request.path,
            &input.name,
            input.duration_seconds,
            "adjustingBitrate",
            2,
            &mut report,
        )?;
        verified = validate_target_output(&temporary, ffprobe, ffmpeg, library_dir)?;
        actual_size = measure(2, &temporary)?;
    }

    let target_exceeded = target_exceeded_by_five_percent(actual_size, target_size);
    let pending_confirmation = target_exceeded && attempts == 2;
    let final_output = if pending_confirmation {
        cleanup.keep(&temporary);
        temporary.clone()
    } else {
        let output = unique_output_path(&destination, &base_name, "mp4");
        fs::rename(&temporary, &output).map_err(|error| {
            VideoCompressionError::new("destination_inaccessible", error.to_string())
        })?;
        cleanup.keep(&temporary);
        output
    };
    let saved_bytes = input.size.saturating_sub(actual_size);
    let no_size_reduction = actual_size >= input.size;
    let reduction_percent = if no_size_reduction || input.size == 0 {
        0.0
    } else {
        saved_bytes as f64 / input.size as f64 * 100.0
    };
    report(VideoCompressionProgress {
        path: request.path,
        name: input.name,
        processed_seconds: input.duration_seconds,
        duration_seconds: input.duration_seconds,
        percent: 100.0,
        phase: if attempts == 2 {
            "adjustingBitrate".into()
        } else {
            "encodingAttempt1".into()
        },
        attempt: attempts,
    });

    Ok(VideoCompressionResult {
        original_size: input.size,
        compressed_size: actual_size,
        saved_bytes,
        reduction_percent,
        duration_seconds: verified.duration_seconds,
        width: verified.width,
        height: verified.height,
        video_codec: verified.video_codec,
        audio_codec: verified.audio_codec,
        output_path: final_output.to_string_lossy().into_owned(),
        output_name: if pending_confirmation {
            format!("{base_name}.mp4")
        } else {
            final_output
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("video-target.mp4")
                .to_owned()
        },
        no_size_reduction,
        quality: None,
        encoding_setting: format!(
            "OpenH264 bitrate mode · {:.0} kbps{}",
            corrected_bitrate.unwrap_or(initial.video_kbps),
            if corrected_bitrate.is_some() {
                " corrected"
            } else {
                ""
            }
        ),
        estimated_size: Some(target_size),
        estimate_difference: Some(actual_size as i64 - target_size as i64),
        target_size: Some(target_size),
        initial_video_bitrate_kbps: Some(initial.video_kbps),
        corrected_video_bitrate_kbps: corrected_bitrate,
        attempts,
        target_exceeded,
        pending_confirmation,
        original_width: input.width,
        original_height: input.height,
        resolution_reduced_automatically: resolution.reduced,
        resolution_reduction_reason: resolution
            .reduced
            .then(|| "bitrate_quality_threshold".to_owned()),
        pending_final_base: pending_confirmation.then_some(base_name),
    })
}

fn target_mp4_ffmpeg_args(
    request: &VideoCompressionRequest,
    input: &Path,
    output: &Path,
    video_kbps: f64,
    resolution: Option<(u64, u64)>,
) -> Vec<OsString> {
    let bitrate = format!("{}k", video_kbps.floor().max(1.0) as u64);
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
    if let Some((width, height)) = resolution {
        args.extend([
            "-vf".into(),
            format!(
                "scale={width}:{height}:force_original_aspect_ratio=decrease:force_divisible_by=2"
            )
            .into(),
        ]);
    }
    args.extend([
        "-c:v".into(),
        "libopenh264".into(),
        "-rc_mode".into(),
        "bitrate".into(),
        "-b:v".into(),
        bitrate.clone().into(),
        "-maxrate".into(),
        bitrate.into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-profile:v".into(),
        "main".into(),
    ]);
    if !request.remove_audio {
        args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "160k".into()]);
    }
    args.extend([
        "-movflags".into(),
        "+faststart".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        output.as_os_str().to_owned(),
    ]);
    args
}

#[allow(clippy::too_many_arguments)]
fn run_target_attempt<F>(
    ffmpeg: &Path,
    library_dir: &Path,
    state: &VideoCompressionState,
    args: &[OsString],
    input_path: &str,
    input_name: &str,
    duration_seconds: f64,
    phase: &str,
    attempt: u8,
    report: &mut F,
) -> Result<(), VideoCompressionError>
where
    F: FnMut(VideoCompressionProgress),
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
    let mut child = command
        .spawn()
        .map_err(|error| VideoCompressionError::new("ffmpeg_unavailable", error.to_string()))?;
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
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(value) = line.strip_prefix("out_time_us=") {
            if let Ok(microseconds) = value.parse::<f64>() {
                let processed = (microseconds / 1_000_000.0).max(0.0);
                report(VideoCompressionProgress {
                    path: input_path.to_owned(),
                    name: input_name.to_owned(),
                    processed_seconds: processed,
                    duration_seconds,
                    percent: (processed / duration_seconds * 100.0).clamp(0.0, 99.9),
                    phase: phase.to_owned(),
                    attempt,
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
    if state.inner.cancel_requested.swap(false, Ordering::SeqCst) {
        return Err(VideoCompressionError::new(
            "compression_cancelled",
            "Video compression was cancelled.",
        ));
    }
    if !status.success() {
        return Err(classify_ffmpeg_error(&stderr_message));
    }
    Ok(())
}

fn validate_target_output(
    path: &Path,
    ffprobe: &Path,
    ffmpeg: &Path,
    library_dir: &Path,
) -> Result<crate::video_tools::VideoInfo, VideoCompressionError> {
    let size = fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if size == 0 {
        return Err(VideoCompressionError::new(
            "output_empty",
            "FFmpeg created an empty output file.",
        ));
    }
    inspect_video_with_tools(path, ffprobe, ffmpeg, library_dir)
        .map_err(|error| VideoCompressionError::new("output_invalid", error.message))
}

fn target_exceeded_by_five_percent(actual: u64, target: u64) -> bool {
    actual as f64 > target as f64 * 1.05
}

fn target_size_label(bytes: u64) -> String {
    let (value, unit) = if bytes >= 1024 * 1024 * 1024 {
        (bytes as f64 / (1024.0 * 1024.0 * 1024.0), "gb")
    } else if bytes >= 1024 * 1024 {
        (bytes as f64 / (1024.0 * 1024.0), "mb")
    } else {
        (bytes as f64 / 1024.0, "kb")
    };
    let formatted = if (value - value.round()).abs() < 0.001 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    };
    format!("{}{}", formatted.replace('.', "-"), unit)
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

fn ffmpeg_args(
    request: &VideoCompressionRequest,
    selected_quality: Option<u8>,
    webm_encoder: Option<WebmEncoder>,
    input: &Path,
    output: &Path,
) -> (Vec<OsString>, String) {
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

    let encoding_setting = match request.output_format.as_str() {
        "mp4" => {
            // libx264 is intentionally absent from the LGPL build. OpenH264 quality mode with
            // global_quality 24 is the closest available equivalent to CRF 24; it has no presets.
            args.extend(["-c:v".into(), "libopenh264".into()]);
            if let Some(quality) = selected_quality {
                let crf_target = quality_to_h264_crf(quality);
                // OpenH264 exposes no CRF or preset. Equal QP bounds in quality mode are
                // its supported constant-quality equivalent and preserve the requested curve.
                args.extend([
                    "-rc_mode".into(),
                    "quality".into(),
                    "-qmin".into(),
                    crf_target.to_string().into(),
                    "-qmax".into(),
                    crf_target.to_string().into(),
                ]);
            } else {
                args.extend([
                    "-rc_mode".into(),
                    "quality".into(),
                    "-global_quality".into(),
                    RECOMMENDED_QUALITY.to_string().into(),
                ]);
            }
            args.extend(["-pix_fmt".into(), "yuv420p".into()]);
            if !request.remove_audio {
                args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "160k".into()]);
            }
            args.extend(["-movflags".into(), "+faststart".into()]);
            if let Some(quality) = selected_quality {
                let crf_target = quality_to_h264_crf(quality);
                format!("OpenH264 fixed QP · qmin/qmax {crf_target} · CRF target {crf_target} · quality {quality}")
            } else {
                format!("OpenH264 quality mode · global_quality {RECOMMENDED_QUALITY}")
            }
        }
        "webm" => {
            let encoder = webm_encoder.unwrap_or(WebmEncoder::SvtAv1);
            let crf = selected_quality
                .map(quality_to_webm_crf)
                .unwrap_or(WEBM_CRF);
            match encoder {
                WebmEncoder::SvtAv1 => args.extend([
                    "-c:v".into(),
                    "libsvtav1".into(),
                    "-crf".into(),
                    crf.to_string().into(),
                    "-preset".into(),
                    WEBM_PRESET.to_string().into(),
                    "-pix_fmt".into(),
                    "yuv420p".into(),
                ]),
                WebmEncoder::Vp9 => args.extend([
                    "-c:v".into(),
                    "libvpx-vp9".into(),
                    "-crf".into(),
                    crf.to_string().into(),
                    "-b:v".into(),
                    "0".into(),
                    "-deadline".into(),
                    "good".into(),
                    "-cpu-used".into(),
                    "2".into(),
                    "-pix_fmt".into(),
                    "yuv420p".into(),
                ]),
            }
            if !request.remove_audio {
                args.extend([
                    "-c:a".into(),
                    "libopus".into(),
                    "-b:a".into(),
                    "128k".into(),
                ]);
            }
            match encoder {
                WebmEncoder::SvtAv1 => {
                    format!("SVT-AV1 constant quality · CRF {crf} · preset {WEBM_PRESET}")
                }
                WebmEncoder::Vp9 => format!("VP9 constant quality · CRF {crf} · bitrate 0"),
            }
        }
        _ => String::new(),
    };
    args.extend([
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        output.as_os_str().to_owned(),
    ]);
    (args, encoding_setting)
}

fn interpolate_quality(quality: u8, points: &[(u8, u8)]) -> u8 {
    let quality = quality.clamp(1, 100);
    for pair in points.windows(2) {
        let (left_quality, left_value) = pair[0];
        let (right_quality, right_value) = pair[1];
        if quality <= right_quality {
            let progress = (quality - left_quality) as f64 / (right_quality - left_quality) as f64;
            return (left_value as f64 + progress * (right_value as f64 - left_value as f64)).round()
                as u8;
        }
    }
    points.last().map(|point| point.1).unwrap_or(quality)
}

fn quality_to_h264_crf(quality: u8) -> u8 {
    interpolate_quality(quality, &[(1, 35), (50, 26), (70, 23), (85, 20), (100, 17)])
}

fn quality_to_webm_crf(quality: u8) -> u8 {
    interpolate_quality(quality, &[(1, 50), (50, 36), (70, 30), (85, 24), (100, 18)])
}

fn detect_webm_encoder(
    ffmpeg: &Path,
    library_dir: &Path,
) -> Result<WebmEncoder, VideoCompressionError> {
    let output = Command::new(ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .env("PATH", path_with_runtime_libraries(library_dir))
        .output()
        .map_err(|error| VideoCompressionError::new("ffmpeg_unavailable", error.to_string()))?;
    let encoders = String::from_utf8_lossy(&output.stdout);
    if encoders.contains("libsvtav1") {
        Ok(WebmEncoder::SvtAv1)
    } else if encoders.contains("libvpx-vp9") {
        Ok(WebmEncoder::Vp9)
    } else {
        Err(VideoCompressionError::new(
            "unsupported_codec",
            "The bundled FFmpeg has no supported AV1 or VP9 WebM encoder.",
        ))
    }
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

    fn create_resolution_source(path: &Path, width: u64, height: u64) {
        let source = format!("testsrc2=size={width}x{height}:rate=30");
        let output = Command::new(bundled_tool("ffmpeg"))
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                &source,
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=700:sample_rate=44100",
                "-t",
                "2",
                "-c:v",
                "mpeg4",
                "-c:a",
                "aac",
                "-shortest",
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

    fn create_silent_source(path: &Path) {
        let output = Command::new(bundled_tool("ffmpeg"))
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=12",
                "-t",
                "1",
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
            quality: None,
            estimated_size: None,
            target_size_bytes: None,
            allow_automatic_resolution_reduction: None,
        }
    }

    fn target_request(
        path: &Path,
        destination: &Path,
        output_format: &str,
        quality: u8,
    ) -> VideoCompressionRequest {
        let mut request = request(path, destination, output_format);
        request.mode = "targetQuality".into();
        request.quality = Some(quality);
        request.estimated_size = Some(42_000);
        request
    }

    fn target_size_request(
        path: &Path,
        destination: &Path,
        target_size_bytes: u64,
    ) -> VideoCompressionRequest {
        let mut request = request(path, destination, "mp4");
        request.mode = "targetFileSize".into();
        request.target_size_bytes = Some(target_size_bytes);
        request.allow_automatic_resolution_reduction = Some(false);
        request
    }

    #[test]
    fn target_mp4_arguments_use_only_supported_openh264_bitrate_options() {
        let request = target_size_request(Path::new("C:\\input.mp4"), Path::new("C:\\"), 1_000_000);
        let joined = target_mp4_ffmpeg_args(
            &request,
            Path::new("C:\\input.mp4"),
            Path::new("C:\\output.mp4"),
            640.9,
            None,
        )
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
        assert!(joined.contains("-c:v libopenh264 -rc_mode bitrate -b:v 640k -maxrate 640k"));
        assert!(joined.contains("-pix_fmt yuv420p -profile:v main"));
        assert!(joined.contains("-c:a aac -b:a 160k"));
        assert!(joined.contains("-movflags +faststart"));
        assert!(!joined.contains("-pass"));
        assert!(!joined.contains("passlogfile"));
        assert!(!joined.contains("preset"));
        assert!(!joined.contains(" -r "));
        assert!(!joined.contains("scale="));

        let scaled = target_mp4_ffmpeg_args(
            &request,
            Path::new("C:\\input.mp4"),
            Path::new("C:\\scaled.mp4"),
            640.9,
            Some((1280, 720)),
        )
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
        assert!(scaled.contains(
            "-vf scale=1280:720:force_original_aspect_ratio=decrease:force_divisible_by=2"
        ));
        assert!(!scaled.contains(" -r "));
    }

    #[test]
    fn target_mp4_automatic_resolution_reduction_encodes_landscape_and_portrait_without_upscaling()
    {
        let temp = tempfile::tempdir().unwrap();
        let output_dir = temp.path().join("outputs");
        fs::create_dir(&output_dir).unwrap();
        let state = VideoCompressionState::default();
        let target = 230 * 1024;

        for (name, width, height, expected) in [
            ("landscape", 1280, 720, (640, 360)),
            ("portrait", 720, 1280, (360, 640)),
        ] {
            let source = temp.path().join(format!("{name}.mp4"));
            create_resolution_source(&source, width, height);
            let original = fs::read(&source).unwrap();

            let unchanged = compress_with_tools(
                target_size_request(&source, &output_dir, target),
                &bundled_tool("ffmpeg"),
                &bundled_tool("ffprobe"),
                &library_dir(),
                &state,
                |_| {},
            )
            .unwrap();
            assert_eq!((unchanged.width, unchanged.height), (width, height));
            assert!(!unchanged.resolution_reduced_automatically);

            let mut automatic_request = target_size_request(&source, &output_dir, target);
            automatic_request.allow_automatic_resolution_reduction = Some(true);
            let reduced = compress_with_tools(
                automatic_request,
                &bundled_tool("ffmpeg"),
                &bundled_tool("ffprobe"),
                &library_dir(),
                &state,
                |_| {},
            )
            .unwrap();
            assert_eq!((reduced.width, reduced.height), expected);
            assert_eq!(
                (reduced.original_width, reduced.original_height),
                (width, height)
            );
            assert!(reduced.resolution_reduced_automatically);
            assert_eq!(
                reduced.resolution_reduction_reason.as_deref(),
                Some("bitrate_quality_threshold")
            );
            assert_eq!(fs::read(&source).unwrap(), original);
            eprintln!(
                "{name}: target={target} bytes, disabled={}x{} / {} bytes, enabled={}x{} / {} bytes, attempts={}",
                unchanged.width,
                unchanged.height,
                unchanged.compressed_size,
                reduced.width,
                reduced.height,
                reduced.compressed_size,
                reduced.attempts
            );
        }
    }

    #[test]
    fn target_helpers_limit_correction_to_a_real_size_excess_and_create_expected_names() {
        assert!(!target_exceeded_by_five_percent(1_050_000, 1_000_000));
        assert!(target_exceeded_by_five_percent(1_050_001, 1_000_000));
        let corrected = corrected_video_bitrate_kbps(1_000.0, 1_000_000, 1_200_000).unwrap();
        assert!((corrected - 808.333_333).abs() < 0.001);
        assert!(target_exceeded_by_five_percent(1_100_000, 1_000_000));
        assert_eq!(target_size_label(25 * 1024 * 1024), "25mb");
        assert_eq!(target_size_label(1536 * 1024), "1-5mb");
    }

    #[test]
    fn target_mp4_really_encodes_audio_silent_unique_valid_outputs_and_preserves_originals() {
        let temp = tempfile::tempdir().unwrap();
        let output_dir = temp.path().join("outputs");
        fs::create_dir(&output_dir).unwrap();
        let source = temp.path().join("video.mp4");
        create_source(&source, "mp4");
        let original = fs::read(&source).unwrap();
        let state = VideoCompressionState::default();
        let target = 96 * 1024;

        let first = compress_with_tools(
            target_size_request(&source, &output_dir, target),
            &bundled_tool("ffmpeg"),
            &bundled_tool("ffprobe"),
            &library_dir(),
            &state,
            |_| {},
        )
        .unwrap();
        let duplicate = compress_with_tools(
            target_size_request(&source, &output_dir, target),
            &bundled_tool("ffmpeg"),
            &bundled_tool("ffprobe"),
            &library_dir(),
            &state,
            |_| {},
        )
        .unwrap();
        let mut muted_request = target_size_request(&source, &output_dir, target);
        muted_request.remove_audio = true;
        let muted = compress_with_tools(
            muted_request,
            &bundled_tool("ffmpeg"),
            &bundled_tool("ffprobe"),
            &library_dir(),
            &state,
            |_| {},
        )
        .unwrap();

        assert_eq!(first.output_name, "video-target-96kb.mp4");
        assert_eq!(duplicate.output_name, "video-target-96kb-2.mp4");
        assert_eq!(first.video_codec, "H264");
        assert_eq!(first.audio_codec.as_deref(), Some("AAC"));
        assert_eq!(muted.audio_codec, None);
        assert!(first.compressed_size > 0 && first.attempts <= 2);
        assert_eq!(first.target_size, Some(target));
        let verified = inspect_video_with_tools(
            Path::new(&first.output_path),
            &bundled_tool("ffprobe"),
            &bundled_tool("ffmpeg"),
            &library_dir(),
        )
        .unwrap();
        assert!(verified.duration_seconds > 0.0 && verified.width == 320 && verified.height == 180);
        assert_eq!(fs::read(&source).unwrap(), original);
        assert!(fs::read_dir(&output_dir).unwrap().all(|entry| {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            !name.contains(".tmp.") && !name.contains("passlog")
        }));
        eprintln!(
            "Target 96 KiB -> {} bytes in {} attempt(s)",
            first.compressed_size, first.attempts
        );
    }

    #[test]
    fn target_mp4_compares_several_real_targets_and_runs_at_most_one_correction() {
        let temp = tempfile::tempdir().unwrap();
        let output_dir = temp.path().join("outputs");
        fs::create_dir(&output_dir).unwrap();
        let source = temp.path().join("sizes.mp4");
        create_source(&source, "mp4");
        let original = fs::read(&source).unwrap();
        let state = VideoCompressionState::default();
        let mut measured = Vec::new();

        for (target_kib, remove_audio) in [
            (15_u64, true),
            (35, false),
            (40, false),
            (48, false),
            (64, false),
            (96, false),
        ] {
            let mut request = target_size_request(&source, &output_dir, target_kib * 1024);
            request.remove_audio = remove_audio;
            let result = compress_with_tools(
                request,
                &bundled_tool("ffmpeg"),
                &bundled_tool("ffprobe"),
                &library_dir(),
                &state,
                |_| {},
            )
            .unwrap();
            assert!((1..=2).contains(&result.attempts));
            assert_eq!(
                result.corrected_video_bitrate_kbps.is_some(),
                result.attempts == 2
            );
            assert!(Path::new(&result.output_path).is_file());
            measured.push((
                target_kib,
                result.compressed_size,
                result.attempts,
                result.target_exceeded,
            ));
            if result.pending_confirmation {
                fs::remove_file(&result.output_path).unwrap();
            }
        }
        assert_eq!(fs::read(&source).unwrap(), original);
        eprintln!("Real Target File Size comparisons (KiB target, bytes actual, attempts, exceeded): {measured:?}");
        assert!(measured.iter().any(|entry| entry.2 == 1));
        assert!(measured.iter().all(|entry| entry.2 <= 2));
    }

    #[test]
    fn target_mp4_cancellation_cleans_temporary_output_in_either_attempt() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("long-target.mp4");
        create_long_source(&source);
        let original = fs::read(&source).unwrap();

        for attempt in [1_u8, 2] {
            let output = temp.path().join(format!(".attempt-{attempt}.tmp.mp4"));
            let state = VideoCompressionState::default();
            let worker_state = state.clone();
            let worker_source = source.clone();
            let worker_output = output.clone();
            let phase = if attempt == 1 {
                "encodingAttempt1"
            } else {
                "adjustingBitrate"
            };
            let worker = thread::spawn(move || {
                let request = target_size_request(
                    &worker_source,
                    worker_source.parent().unwrap(),
                    700 * 1024,
                );
                let args = target_mp4_ffmpeg_args(
                    &request,
                    &worker_source,
                    &worker_output,
                    600.0,
                    Some((640, 360)),
                );
                let mut cleanup = TemporaryOutputs::new();
                cleanup.track(worker_output);
                run_target_attempt(
                    &bundled_tool("ffmpeg"),
                    &library_dir(),
                    &worker_state,
                    &args,
                    &request.path,
                    "long-target.mp4",
                    8.0,
                    phase,
                    attempt,
                    &mut |_| {},
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
                thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(killed, "Attempt {attempt} did not become active");
            let error = worker.join().unwrap().unwrap_err();
            assert_eq!(error.code, "compression_cancelled");
            assert!(!output.exists());
        }
        assert_eq!(fs::read(&source).unwrap(), original);
        assert!(fs::read_dir(temp.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("passlog")));
    }

    #[test]
    fn target_mp4_executes_one_full_correction_and_requires_consent_if_excess_persists() {
        let temp = tempfile::tempdir().unwrap();
        let output_dir = temp.path().join("outputs");
        fs::create_dir(&output_dir).unwrap();
        let source = temp.path().join("corrective.mp4");
        create_resolution_source(&source, 1280, 720);
        let original = fs::read(&source).unwrap();
        let state = VideoCompressionState::default();
        let target = 230 * 1024;
        let mut corrective_request = target_size_request(&source, &output_dir, target);
        corrective_request.allow_automatic_resolution_reduction = Some(true);

        let corrected = compress_target_mp4_with_measure(
            corrective_request.clone(),
            &bundled_tool("ffmpeg"),
            &bundled_tool("ffprobe"),
            &library_dir(),
            &state,
            |_| {},
            |attempt, path| {
                Ok(if attempt == 1 {
                    target * 110 / 100
                } else {
                    fs::metadata(path).unwrap().len()
                })
            },
        )
        .unwrap();
        assert_eq!(corrected.attempts, 2);
        assert!(corrected.corrected_video_bitrate_kbps.is_some());
        assert!(!corrected.pending_confirmation);
        assert!(!corrected.target_exceeded);
        assert_eq!((corrected.width, corrected.height), (640, 360));
        assert!(corrected.resolution_reduced_automatically);
        assert!(Path::new(&corrected.output_path).is_file());

        let persistent = compress_target_mp4_with_measure(
            corrective_request,
            &bundled_tool("ffmpeg"),
            &bundled_tool("ffprobe"),
            &library_dir(),
            &state,
            |_| {},
            |attempt, _path| {
                Ok(if attempt == 1 {
                    target * 110 / 100
                } else {
                    target * 120 / 100
                })
            },
        )
        .unwrap();
        assert_eq!(persistent.attempts, 2);
        assert!(persistent.target_exceeded);
        assert!(persistent.pending_confirmation);
        assert_eq!((persistent.width, persistent.height), (640, 360));
        assert!(Path::new(&persistent.output_path).is_file());
        assert!(Path::new(&persistent.output_path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("attempt-2"));
        assert!(fs::read_dir(&output_dir).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("attempt-1")));
        fs::remove_file(&persistent.output_path).unwrap();
        assert_eq!(fs::read(&source).unwrap(), original);
    }

    #[test]
    fn target_quality_curve_matches_all_required_control_points() {
        assert_eq!(quality_to_h264_crf(1), 35);
        assert_eq!(quality_to_h264_crf(50), 26);
        assert_eq!(quality_to_h264_crf(70), 23);
        assert_eq!(quality_to_h264_crf(85), 20);
        assert_eq!(quality_to_h264_crf(100), 17);
        assert!(quality_to_h264_crf(30) > quality_to_h264_crf(70));
        assert!(quality_to_h264_crf(70) > quality_to_h264_crf(95));
    }

    #[test]
    fn target_quality_uses_only_options_supported_by_each_encoder() {
        let input = Path::new("C:\\input.mp4");
        let mp4 = target_request(input, Path::new("C:\\"), "mp4", 70);
        let (args, setting) = ffmpeg_args(&mp4, Some(70), None, input, Path::new("C:\\output.mp4"));
        let joined = args
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains("-c:v libopenh264 -rc_mode quality -qmin 23 -qmax 23"));
        assert!(joined.contains("-pix_fmt yuv420p"));
        assert!(joined.contains("-movflags +faststart"));
        assert!(joined.contains("-c:a aac"));
        assert!(!joined.contains("-preset medium"));
        assert!(setting.contains("qmin/qmax 23"));

        let webm = target_request(input, Path::new("C:\\"), "webm", 85);
        let (svt_args, _) = ffmpeg_args(
            &webm,
            Some(85),
            Some(WebmEncoder::SvtAv1),
            input,
            Path::new("C:\\output.webm"),
        );
        let svt = svt_args
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(svt.contains("-c:v libsvtav1 -crf 24 -preset 8"));
        assert!(!svt.contains("-b:v 0"));
        assert!(svt.contains("-c:a libopus"));

        let (vp9_args, _) = ffmpeg_args(
            &webm,
            Some(85),
            Some(WebmEncoder::Vp9),
            input,
            Path::new("C:\\output.webm"),
        );
        let vp9 = vp9_args
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(vp9.contains("-c:v libvpx-vp9 -crf 24 -b:v 0"));
        assert!(!vp9.contains("-preset"));
    }

    #[test]
    fn recommended_arguments_implement_supported_options_without_changing_framerate() {
        let input = Path::new("C:\\input.mp4");
        let output = Path::new("C:\\output.mp4");
        let mut req = request(input, Path::new("C:\\"), "mp4");
        req.preserve_metadata = false;
        let joined = ffmpeg_args(&req, None, None, input, output)
            .0
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
        let joined = ffmpeg_args(
            &req,
            None,
            Some(WebmEncoder::SvtAv1),
            input,
            Path::new("C:\\output.webm"),
        )
        .0
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
    fn target_quality_compresses_real_files_at_30_70_95_and_handles_audio_options() {
        let temp = tempfile::tempdir().unwrap();
        let output_dir = temp.path().join("outputs");
        fs::create_dir(&output_dir).unwrap();
        let source = temp.path().join("quality-source.mp4");
        let silent = temp.path().join("silent.mp4");
        create_source(&source, "mp4");
        create_silent_source(&silent);
        let original = fs::read(&source).unwrap();
        let silent_original = fs::read(&silent).unwrap();
        let state = VideoCompressionState::default();
        let mut sizes = Vec::new();

        for quality in [30, 70, 95] {
            let result = compress_with_tools(
                target_request(&source, &output_dir, "mp4", quality),
                &bundled_tool("ffmpeg"),
                &bundled_tool("ffprobe"),
                &library_dir(),
                &state,
                |_| {},
            )
            .unwrap();
            assert_eq!(result.quality, Some(quality));
            assert_eq!(result.video_codec, "H264");
            assert_eq!(result.audio_codec.as_deref(), Some("AAC"));
            assert!(result.encoding_setting.contains("OpenH264 fixed QP"));
            assert_eq!(result.estimated_size, Some(42_000));
            assert!(Path::new(&result.output_path).is_file());
            let verified = inspect_video_with_tools(
                Path::new(&result.output_path),
                &bundled_tool("ffprobe"),
                &bundled_tool("ffmpeg"),
                &library_dir(),
            )
            .unwrap();
            assert!(verified.duration_seconds > 0.0 && verified.width > 0);
            sizes.push((quality, result.compressed_size));
        }

        let silent_result = compress_with_tools(
            target_request(&silent, &output_dir, "webm", 70),
            &bundled_tool("ffmpeg"),
            &bundled_tool("ffprobe"),
            &library_dir(),
            &state,
            |_| {},
        )
        .unwrap();
        assert_eq!(silent_result.audio_codec, None);
        assert_eq!(silent_result.video_codec, "AV1");

        let mut remove_audio = target_request(&source, &output_dir, "mp4", 70);
        remove_audio.remove_audio = true;
        let muted = compress_with_tools(
            remove_audio,
            &bundled_tool("ffmpeg"),
            &bundled_tool("ffprobe"),
            &library_dir(),
            &state,
            |_| {},
        )
        .unwrap();
        assert_eq!(muted.audio_codec, None);
        assert_eq!(fs::read(&source).unwrap(), original);
        assert_eq!(fs::read(&silent).unwrap(), silent_original);
        assert!(sizes[0].1 < sizes[1].1 && sizes[1].1 < sizes[2].1);
        eprintln!("Target Quality MP4 sizes: {sizes:?}");
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
                target_request(&worker_source, &worker_output, "webm", 70),
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
