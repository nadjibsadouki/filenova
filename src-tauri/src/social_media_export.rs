use std::{
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, Window};

use crate::{
    video_conversion_plan::detect_video_conversion_capabilities,
    video_tools::{
        find_sidecar, inspect_video_with_tools, path_with_runtime_libraries, runtime_library_dir,
    },
};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Default)]
pub struct SocialExportState {
    inner: Arc<SocialExportInner>,
}

#[derive(Default)]
struct SocialExportInner {
    running: AtomicBool,
    cancel_requested: AtomicBool,
    child: Mutex<Option<Child>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SocialExportRequest {
    path: String,
    destination_dir: Option<String>,
    output_name: String,
    target_width: i64,
    target_height: i64,
    method: String,
    stretch_confirmed: bool,
    background: String,
    custom_color: Option<String>,
    crop_position: String,
    crop_x: f64,
    crop_y: f64,
    preserve_frame_rate: bool,
    frames_per_second: f64,
    preserve_audio: bool,
    preserve_metadata: bool,
    quality: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SocialExportProgress {
    processed_seconds: f64,
    duration_seconds: f64,
    percent: f64,
    phase: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SocialExportResult {
    output_path: String,
    output_name: String,
    width: u64,
    height: u64,
    frames_per_second: f64,
    video_codec: String,
    audio_codec: Option<String>,
    final_size: u64,
    processing_duration_seconds: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SocialExportError {
    code: &'static str,
    message: String,
}

impl SocialExportError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[tauri::command]
pub async fn export_social_media_video(
    app: AppHandle,
    window: Window,
    state: State<'_, SocialExportState>,
    request: SocialExportRequest,
) -> Result<SocialExportResult, SocialExportError> {
    if state.inner.running.swap(true, Ordering::SeqCst) {
        return Err(SocialExportError::new(
            "social_export_busy",
            "Another social media export is already running.",
        ));
    }
    state.inner.cancel_requested.store(false, Ordering::SeqCst);
    let _guard = RunningGuard(state.inner.clone());
    let ffmpeg = find_sidecar(&app, "ffmpeg")
        .map_err(|error| SocialExportError::new("ffmpeg_unavailable", error))?;
    let ffprobe = find_sidecar(&app, "ffprobe")
        .map_err(|error| SocialExportError::new("ffprobe_unavailable", error))?;
    let library_dir = runtime_library_dir(&app);
    let export_state = state.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        export_with_tools_state(
            request,
            &ffmpeg,
            &ffprobe,
            &library_dir,
            &export_state,
            |progress| {
                let _ = window.emit("social-media-export-progress", progress);
            },
        )
    })
    .await
    .map_err(|error| SocialExportError::new("social_export_failed", error.to_string()))??;
    app.asset_protocol_scope()
        .allow_file(Path::new(&result.output_path))
        .map_err(|error| SocialExportError::new("social_output_invalid", error.to_string()))?;
    Ok(result)
}

#[tauri::command]
pub fn cancel_social_media_export(
    state: State<'_, SocialExportState>,
) -> Result<bool, SocialExportError> {
    let mut active =
        state.inner.child.lock().map_err(|_| {
            SocialExportError::new("social_export_failed", "Process lock poisoned.")
        })?;
    if !state.inner.running.load(Ordering::SeqCst) {
        return Ok(false);
    }
    state.inner.cancel_requested.store(true, Ordering::SeqCst);
    if let Some(child) = active.as_mut() {
        child
            .kill()
            .map_err(|error| SocialExportError::new("social_cancel_failed", error.to_string()))?;
    }
    Ok(true)
}

#[cfg(test)]
fn export_with_tools(
    request: SocialExportRequest,
    ffmpeg: &Path,
    ffprobe: &Path,
    library_dir: &Path,
) -> Result<SocialExportResult, SocialExportError> {
    let state = SocialExportState::default();
    export_with_tools_state(request, ffmpeg, ffprobe, library_dir, &state, |_| {})
}

fn export_with_tools_state<F>(
    request: SocialExportRequest,
    ffmpeg: &Path,
    ffprobe: &Path,
    library_dir: &Path,
    state: &SocialExportState,
    mut report: F,
) -> Result<SocialExportResult, SocialExportError>
where
    F: FnMut(SocialExportProgress),
{
    let started = Instant::now();
    let input_path = PathBuf::from(&request.path)
        .canonicalize()
        .map_err(|error| SocialExportError::new("video_not_found", error.to_string()))?;
    let input = inspect_video_with_tools(&input_path, ffprobe, ffmpeg, library_dir)
        .map_err(|error| SocialExportError::new(error.code, error.message))?;
    if input.duration_seconds <= 0.0 {
        return Err(SocialExportError::new(
            "social_output_invalid",
            "The source video duration is missing or invalid.",
        ));
    }
    let capabilities = detect_video_conversion_capabilities(ffmpeg, library_dir)
        .map_err(|error| SocialExportError::new("ffmpeg_unavailable", error))?;
    if !capabilities.openh264
        || (request.preserve_audio && input.audio_codec.is_some() && !capabilities.aac)
    {
        return Err(SocialExportError::new(
            "social_encoder_unavailable",
            "The required OpenH264 or AAC encoder is unavailable in bundled FFmpeg.",
        ));
    }
    let (target_width, target_height) = validate_dimensions(
        request.target_width,
        request.target_height,
        input.width,
        input.height,
        &request.method,
    )?;
    if request.method == "stretch" && !request.stretch_confirmed {
        return Err(SocialExportError::new(
            "stretch_confirmation_required",
            "Stretch must be explicitly confirmed because it can distort the video.",
        ));
    }
    let background_color = if request.method == "fit" {
        resolve_fit_background(&request.background, request.custom_color.as_deref())?
    } else {
        "0x000000".into()
    };
    let crop_position = if request.method == "fill" {
        resolve_crop_position(&request.crop_position, request.crop_x, request.crop_y)?
    } else {
        CropPosition {
            x_percent: 50.0,
            y_percent: 50.0,
        }
    };
    let frames_per_second = if request.preserve_frame_rate {
        input.frames_per_second
    } else {
        validate_frame_rate(request.frames_per_second)?
    };
    let quality = resolve_quality(&request.quality)?;
    let destination = destination_directory(&input_path, request.destination_dir.as_deref())?;
    let stem = output_stem(&request.output_name)?;
    let final_output = unique_output_path(&destination, &stem);
    let temporary_output = destination.join(format!(".{stem}.{}.tmp.mp4", unique_nonce()));
    verify_destination_writable(&temporary_output)?;
    let mut temporary = TemporaryOutput(Some(temporary_output.clone()));
    let arguments = export_arguments(
        &input_path,
        &temporary_output,
        target_width,
        target_height,
        request.preserve_frame_rate,
        frames_per_second,
        request.preserve_audio,
        request.preserve_metadata,
        quality.qp,
        &background_color,
        &request.method,
        crop_position,
    );
    report(SocialExportProgress {
        processed_seconds: 0.0,
        duration_seconds: input.duration_seconds,
        percent: 0.0,
        phase: "preparing".into(),
    });
    run_ffmpeg(
        ffmpeg,
        library_dir,
        state,
        &arguments,
        input.duration_seconds,
        &mut report,
    )?;
    let metadata = fs::metadata(&temporary_output)
        .map_err(|error| SocialExportError::new("social_output_invalid", error.to_string()))?;
    if metadata.len() == 0 {
        return Err(SocialExportError::new(
            "social_output_invalid",
            "FFmpeg created an empty MP4 file.",
        ));
    }
    let validated = inspect_video_with_tools(&temporary_output, ffprobe, ffmpeg, library_dir)
        .map_err(|error| SocialExportError::new("social_output_invalid", error.message))?;
    if validated.width != target_width || validated.height != target_height {
        return Err(SocialExportError::new(
            "social_output_invalid",
            "The exported video dimensions do not match the selected preset.",
        ));
    }
    if validated.video_codec != "H264" || validated.format != "MP4" {
        return Err(SocialExportError::new(
            "social_output_invalid",
            "The exported file is not a valid H.264 MP4 video.",
        ));
    }
    if request.preserve_audio
        && input.audio_codec.is_some()
        && validated.audio_codec.as_deref() != Some("AAC")
    {
        return Err(SocialExportError::new(
            "social_output_invalid",
            "The exported MP4 audio stream is not AAC.",
        ));
    }
    if !request.preserve_audio && validated.audio_codec.is_some() {
        return Err(SocialExportError::new(
            "social_output_invalid",
            "The exported MP4 unexpectedly contains an audio stream.",
        ));
    }
    let duration_tolerance = (2.0 / frames_per_second.max(1.0)).max(0.25);
    if (validated.duration_seconds - input.duration_seconds).abs() > duration_tolerance {
        return Err(SocialExportError::new(
            "social_output_invalid",
            "The exported MP4 duration does not match the source video.",
        ));
    }
    fs::rename(&temporary_output, &final_output)
        .map_err(|error| SocialExportError::new("destination_inaccessible", error.to_string()))?;
    temporary.0 = None;
    report(SocialExportProgress {
        processed_seconds: input.duration_seconds,
        duration_seconds: input.duration_seconds,
        percent: 100.0,
        phase: "preparing".into(),
    });
    Ok(SocialExportResult {
        output_path: final_output.to_string_lossy().into_owned(),
        output_name: final_output
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("video-social.mp4")
            .to_owned(),
        width: validated.width,
        height: validated.height,
        frames_per_second: validated.frames_per_second,
        video_codec: validated.video_codec,
        audio_codec: validated.audio_codec,
        final_size: metadata.len(),
        processing_duration_seconds: started.elapsed().as_secs_f64(),
    })
}

fn validate_dimensions(
    target_width: i64,
    target_height: i64,
    source_width: u64,
    source_height: u64,
    method: &str,
) -> Result<(u64, u64), SocialExportError> {
    if !matches!(method, "fit" | "fill" | "stretch") {
        return Err(SocialExportError::new(
            "invalid_social_method",
            "Unknown social media adaptation method.",
        ));
    }
    if target_width <= 0 || target_height <= 0 || target_width % 2 != 0 || target_height % 2 != 0 {
        return Err(SocialExportError::new(
            "invalid_social_dimensions",
            "MP4 dimensions must be positive even numbers.",
        ));
    }
    let target_width = target_width as u64;
    let target_height = target_height as u64;
    if source_width == 0 || source_height == 0 {
        return Err(SocialExportError::new(
            "invalid_social_dimensions",
            "The source video dimensions are invalid.",
        ));
    }
    Ok((target_width, target_height))
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct CropPosition {
    x_percent: f64,
    y_percent: f64,
}

fn resolve_crop_position(
    position: &str,
    custom_x: f64,
    custom_y: f64,
) -> Result<CropPosition, SocialExportError> {
    let position = match position {
        "center" => CropPosition {
            x_percent: 50.0,
            y_percent: 50.0,
        },
        "top" => CropPosition {
            x_percent: 50.0,
            y_percent: 0.0,
        },
        "bottom" => CropPosition {
            x_percent: 50.0,
            y_percent: 100.0,
        },
        "left" => CropPosition {
            x_percent: 0.0,
            y_percent: 50.0,
        },
        "right" => CropPosition {
            x_percent: 100.0,
            y_percent: 50.0,
        },
        "custom" if custom_x.is_finite() && custom_y.is_finite() => CropPosition {
            x_percent: custom_x.clamp(0.0, 100.0),
            y_percent: custom_y.clamp(0.0, 100.0),
        },
        "custom" => {
            return Err(SocialExportError::new(
                "invalid_crop_position",
                "Custom crop coordinates must be finite numbers.",
            ));
        }
        _ => {
            return Err(SocialExportError::new(
                "invalid_crop_position",
                "Unknown Fill crop position.",
            ));
        }
    };
    Ok(position)
}

fn resolve_fit_background(
    background: &str,
    custom_color: Option<&str>,
) -> Result<String, SocialExportError> {
    match background {
        "black" => Ok("0x000000".into()),
        "white" => Ok("0xFFFFFF".into()),
        "custom" => {
            let value = custom_color.unwrap_or("").trim();
            let hex = value.strip_prefix('#').unwrap_or(value);
            if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(SocialExportError::new(
                    "invalid_social_background",
                    "Custom Fit background must use the #RRGGBB hexadecimal format.",
                ));
            }
            Ok(format!("0x{}", hex.to_ascii_uppercase()))
        }
        "blurred" => Ok("blurred".into()),
        _ => Err(SocialExportError::new(
            "invalid_social_background",
            "Unknown Fit background color.",
        )),
    }
}

fn validate_frame_rate(value: f64) -> Result<f64, SocialExportError> {
    if !value.is_finite() || value <= 0.0 || value > 120.0 {
        return Err(SocialExportError::new(
            "invalid_social_frame_rate",
            "Frame rate must be between 1 and 120 fps.",
        ));
    }
    Ok(value)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SocialQualityProfile {
    qp: u8,
}

fn openh264_quality_value(quality: u8) -> u8 {
    let points = [(1_u8, 35_u8), (50, 26), (70, 23), (85, 20), (100, 17)];
    let quality = quality.clamp(1, 100);
    for pair in points.windows(2) {
        let (left_quality, left_value) = pair[0];
        let (right_quality, right_value) = pair[1];
        if quality <= right_quality {
            let position = (quality - left_quality) as f64 / (right_quality - left_quality) as f64;
            return (left_value as f64 + (right_value as f64 - left_value as f64) * position).round()
                as u8;
        }
    }
    17
}

fn resolve_quality(value: &str) -> Result<SocialQualityProfile, SocialExportError> {
    let quality = match value {
        "smaller" => 30,
        "balanced" => 70,
        "high" => 85,
        "maximum" => 100,
        _ => {
            return Err(SocialExportError::new(
                "invalid_social_quality",
                "Unknown Social Media quality profile.",
            ));
        }
    };
    Ok(SocialQualityProfile {
        qp: openh264_quality_value(quality),
    })
}

fn export_arguments(
    input: &Path,
    output: &Path,
    width: u64,
    height: u64,
    preserve_frame_rate: bool,
    frames_per_second: f64,
    preserve_audio: bool,
    preserve_metadata: bool,
    quality_qp: u8,
    background_color: &str,
    method: &str,
    crop_position: CropPosition,
) -> Vec<String> {
    let mut args = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-v".into(),
        "warning".into(),
        "-n".into(),
        "-i".into(),
        input.to_string_lossy().into_owned(),
    ];
    if method == "stretch" {
        args.extend([
            "-map".into(),
            "0:v:0".into(),
            "-vf".into(),
            format!("scale={width}:{height}:flags=lanczos,format=yuv420p"),
        ]);
    } else if method == "fill" {
        args.extend([
            "-map".into(),
            "0:v:0".into(),
            "-vf".into(),
            format!(
                "scale={width}:{height}:force_original_aspect_ratio=increase:force_divisible_by=2:flags=lanczos,crop={width}:{height}:x='(iw-ow)*{:.6}/100':y='(ih-oh)*{:.6}/100',format=yuv420p",
                crop_position.x_percent, crop_position.y_percent
            ),
        ]);
    } else if background_color == "blurred" {
        args.extend([
            "-filter_complex".into(),
            format!(
                "[0:v]split=2[background_source][foreground_source];[background_source]scale={width}:{height}:force_original_aspect_ratio=increase:force_divisible_by=2:flags=lanczos,crop={width}:{height},gblur=sigma=20:steps=2[background];[foreground_source]scale=w='min(iw\\,{width})':h='min(ih\\,{height})':force_original_aspect_ratio=decrease:force_divisible_by=2:flags=lanczos[foreground];[background][foreground]overlay=(W-w)/2:(H-h)/2:shortest=1,format=yuv420p[video_out]"
            ),
            "-map".into(),
            "[video_out]".into(),
        ]);
    } else {
        args.extend([
            "-map".into(),
            "0:v:0".into(),
            "-vf".into(),
            format!(
                "scale=w='min(iw\\,{width})':h='min(ih\\,{height})':force_original_aspect_ratio=decrease:force_divisible_by=2:flags=lanczos,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:color={background_color}"
            ),
        ]);
    }
    if preserve_audio {
        args.extend(["-map".into(), "0:a:0?".into()]);
    }
    args.extend([
        "-c:v".into(),
        "libopenh264".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-profile:v".into(),
        "main".into(),
        "-rc_mode".into(),
        "quality".into(),
        "-qmin".into(),
        quality_qp.to_string(),
        "-qmax".into(),
        quality_qp.to_string(),
    ]);
    if !preserve_frame_rate {
        args.extend(["-r".into(), format!("{frames_per_second:.6}")]);
    }
    if preserve_audio {
        args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "192k".into()]);
    } else {
        args.push("-an".into());
    }
    args.extend([
        "-map_metadata".into(),
        if preserve_metadata {
            "0".into()
        } else {
            "-1".into()
        },
        "-map_chapters".into(),
        if preserve_metadata {
            "0".into()
        } else {
            "-1".into()
        },
        "-movflags".into(),
        "+faststart".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        output.to_string_lossy().into_owned(),
    ]);
    args
}

fn run_ffmpeg<F>(
    ffmpeg: &Path,
    library_dir: &Path,
    state: &SocialExportState,
    arguments: &[String],
    duration_seconds: f64,
    report: &mut F,
) -> Result<(), SocialExportError>
where
    F: FnMut(SocialExportProgress),
{
    let mut command = Command::new(ffmpeg);
    command
        .args(arguments)
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
        .map_err(|error| SocialExportError::new("ffmpeg_unavailable", error.to_string()))?;
    let stdout = child.stdout.take().ok_or_else(|| {
        SocialExportError::new("social_export_failed", "FFmpeg progress is unavailable.")
    })?;
    let mut stderr = child.stderr.take().ok_or_else(|| {
        SocialExportError::new(
            "social_export_failed",
            "FFmpeg error output is unavailable.",
        )
    })?;
    {
        let mut active = state.inner.child.lock().map_err(|_| {
            SocialExportError::new("social_export_failed", "Process lock poisoned.")
        })?;
        if active.is_some() {
            let _ = child.kill();
            return Err(SocialExportError::new(
                "social_export_busy",
                "Another social media export is already running.",
            ));
        }
        *active = Some(child);
        if state.inner.cancel_requested.load(Ordering::SeqCst) {
            active
                .as_mut()
                .expect("The FFmpeg child was just stored")
                .kill()
                .map_err(|error| {
                    SocialExportError::new("social_cancel_failed", error.to_string())
                })?;
        }
    }
    let stderr_reader = thread::spawn(move || {
        let mut message = String::new();
        let _ = stderr.read_to_string(&mut message);
        message
    });
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(value) = line.strip_prefix("out_time_us=") {
            if let Ok(microseconds) = value.parse::<f64>() {
                let processed_seconds = (microseconds / 1_000_000.0).max(0.0);
                report(SocialExportProgress {
                    processed_seconds,
                    duration_seconds,
                    percent: (processed_seconds / duration_seconds * 100.0).clamp(0.0, 99.9),
                    phase: "preparing".into(),
                });
            }
        }
    }
    let status = state
        .inner
        .child
        .lock()
        .map_err(|_| SocialExportError::new("social_export_failed", "Process lock poisoned."))?
        .take()
        .ok_or_else(|| SocialExportError::new("social_export_failed", "FFmpeg disappeared."))?
        .wait()
        .map_err(|error| SocialExportError::new("social_export_failed", error.to_string()))?;
    let stderr = stderr_reader.join().unwrap_or_default();
    if state.inner.cancel_requested.swap(false, Ordering::SeqCst) {
        return Err(SocialExportError::new(
            "social_export_cancelled",
            "Social media export was cancelled.",
        ));
    }
    if !status.success() {
        let lower = stderr.to_ascii_lowercase();
        let code = if lower.contains("no space left") || lower.contains("disk full") {
            "disk_full"
        } else if lower.contains("permission denied") || lower.contains("access is denied") {
            "destination_inaccessible"
        } else {
            "social_export_failed"
        };
        return Err(SocialExportError::new(code, stderr.trim()));
    }
    Ok(())
}

fn destination_directory(
    input: &Path,
    requested: Option<&str>,
) -> Result<PathBuf, SocialExportError> {
    let directory = requested
        .map(PathBuf::from)
        .unwrap_or_else(|| input.parent().unwrap_or(Path::new(".")).to_path_buf());
    if !directory.is_dir() {
        return Err(SocialExportError::new(
            "destination_inaccessible",
            "The destination folder does not exist.",
        ));
    }
    Ok(directory)
}

fn output_stem(name: &str) -> Result<String, SocialExportError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.contains(['/', '\\']) {
        return Err(SocialExportError::new(
            "invalid_output_name",
            "Enter a valid local MP4 file name.",
        ));
    }
    let stem = Path::new(trimmed)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .trim();
    if stem.is_empty() || matches!(stem, "." | "..") {
        return Err(SocialExportError::new(
            "invalid_output_name",
            "Enter a valid local MP4 file name.",
        ));
    }
    Ok(stem.to_owned())
}

fn unique_output_path(directory: &Path, stem: &str) -> PathBuf {
    let first = directory.join(format!("{stem}.mp4"));
    if !first.exists() {
        return first;
    }
    for index in 2..u32::MAX {
        let candidate = directory.join(format!("{stem}-{index}.mp4"));
        if !candidate.exists() {
            return candidate;
        }
    }
    directory.join(format!("{stem}-{}.mp4", unique_nonce()))
}

fn verify_destination_writable(path: &Path) -> Result<(), SocialExportError> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| SocialExportError::new("destination_inaccessible", error.to_string()))?;
    fs::remove_file(path)
        .map_err(|error| SocialExportError::new("destination_inaccessible", error.to_string()))
}

fn unique_nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        ^ NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed) as u128
}

struct TemporaryOutput(Option<PathBuf>);
impl Drop for TemporaryOutput {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

struct RunningGuard(Arc<SocialExportInner>);
impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::SeqCst);
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

    fn create_video(path: &Path, width: u32, height: u32, audio: bool, duration: f64) {
        create_video_with_options(path, width, height, audio, duration, 15.0, None);
    }

    fn create_video_with_options(
        path: &Path,
        width: u32,
        height: u32,
        audio: bool,
        duration: f64,
        frame_rate: f64,
        title: Option<&str>,
    ) {
        let mut command = Command::new(tool("ffmpeg"));
        command
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!(
                "testsrc=size={width}x{height}:rate={frame_rate:.6}"
            ));
        if audio {
            command.args(["-f", "lavfi", "-i", "sine=frequency=600:sample_rate=44100"]);
        }
        command.args(["-t", &format!("{duration:.3}"), "-c:v", "mpeg4"]);
        if audio {
            command.args(["-c:a", "aac", "-shortest"]);
        } else {
            command.arg("-an");
        }
        if let Some(title) = title {
            command.arg("-metadata").arg(format!("title={title}"));
        }
        let output = command
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

    fn request(
        path: &Path,
        destination: &Path,
        name: &str,
        width: i64,
        height: i64,
    ) -> SocialExportRequest {
        SocialExportRequest {
            path: path.to_string_lossy().into_owned(),
            destination_dir: Some(destination.to_string_lossy().into_owned()),
            output_name: name.into(),
            target_width: width,
            target_height: height,
            method: "fit".into(),
            background: "black".into(),
            custom_color: None,
            crop_position: "center".into(),
            crop_x: 50.0,
            crop_y: 50.0,
            preserve_frame_rate: true,
            frames_per_second: 15.0,
            preserve_audio: true,
            preserve_metadata: false,
            stretch_confirmed: false,
            quality: "balanced".into(),
        }
    }

    #[test]
    fn base_arguments_use_only_lgpl_compatible_mp4_options() {
        let args = export_arguments(
            Path::new("input.mp4"),
            Path::new("output.mp4"),
            1280,
            720,
            false,
            30.0,
            true,
            false,
            23,
            "0x000000",
            "fit",
            CropPosition {
                x_percent: 50.0,
                y_percent: 50.0,
            },
        );
        let joined = args.join(" ");
        for required in [
            "-c:v libopenh264",
            "-pix_fmt yuv420p",
            "-profile:v main",
            "-c:a aac",
            "-movflags +faststart",
            "force_original_aspect_ratio=decrease",
            "force_divisible_by=2",
            "pad=1280:720:(ow-iw)/2:(oh-ih)/2:color=0x000000",
        ] {
            assert!(joined.contains(required), "missing {required}");
        }
        for forbidden in [
            "libx264",
            "-crf",
            "preset medium",
            "-preset",
            "drawbox",
            "drawtext",
            "safe_zone",
            "tiktok",
            "instagram",
            "youtube",
        ] {
            assert!(!joined.contains(forbidden), "forbidden {forbidden}");
        }

        let blurred = export_arguments(
            Path::new("input.mp4"),
            Path::new("output.mp4"),
            1080,
            1920,
            true,
            30.0,
            true,
            false,
            23,
            "blurred",
            "fit",
            CropPosition {
                x_percent: 50.0,
                y_percent: 50.0,
            },
        )
        .join(" ");
        for required in [
            "-filter_complex",
            "split=2",
            "force_original_aspect_ratio=increase",
            "crop=1080:1920",
            "gblur=sigma=20:steps=2",
            "force_original_aspect_ratio=decrease",
            "overlay=(W-w)/2:(H-h)/2:shortest=1",
            "-map [video_out]",
            "-map 0:a:0?",
        ] {
            assert!(blurred.contains(required), "missing {required}");
        }

        let fill = export_arguments(
            Path::new("input.mp4"),
            Path::new("output.mp4"),
            1080,
            1920,
            true,
            30.0,
            true,
            false,
            23,
            "0x000000",
            "fill",
            CropPosition {
                x_percent: 25.0,
                y_percent: 75.0,
            },
        )
        .join(" ");
        for required in [
            "force_original_aspect_ratio=increase",
            "force_divisible_by=2",
            "crop=1080:1920:x='(iw-ow)*25.000000/100':y='(ih-oh)*75.000000/100'",
        ] {
            assert!(fill.contains(required), "missing {required}");
        }

        let stretch = export_arguments(
            Path::new("input.mp4"),
            Path::new("output.mp4"),
            1080,
            1920,
            false,
            24.0,
            false,
            true,
            23,
            "0x000000",
            "stretch",
            CropPosition {
                x_percent: 50.0,
                y_percent: 50.0,
            },
        )
        .join(" ");
        for required in [
            "scale=1080:1920:flags=lanczos,format=yuv420p",
            "-r 24.000000",
            "-an",
            "-map_metadata 0",
            "-map_chapters 0",
        ] {
            assert!(stretch.contains(required), "missing {required}");
        }
        for forbidden in ["pad=", "crop=", "force_original_aspect_ratio"] {
            assert!(!stretch.contains(forbidden), "forbidden {forbidden}");
        }
    }

    #[test]
    fn social_ui_wires_preview_windows_actions_and_keeps_safe_zones_preview_only() {
        let source = include_str!("../../src/pages/SocialMediaToolsPage.tsx");
        for required in [
            "setPreviewResult(true)",
            "convertFileSrc(exportResult.outputPath)",
            "openPath(path)",
            "revealItemInDir(path)",
            "social-safe-zone",
            "socialSafeZonePreviewOnly",
        ] {
            assert!(source.contains(required), "missing UI wiring: {required}");
        }
        let request_start = source
            .find("invoke<SocialExportResult>(\"export_social_media_video\"")
            .expect("Social Media export invoke is missing");
        let result_start = source[request_start..]
            .find("setExportResult({")
            .map(|offset| request_start + offset)
            .expect("Social Media result handling is missing");
        let request_block = &source[request_start..result_start];
        assert!(request_block.contains("quality,"));
        assert!(!request_block.contains("safeZone"));
    }

    #[test]
    fn rejects_invalid_dimensions_and_frame_rates() {
        assert_eq!(
            validate_dimensions(0, 720, 1280, 720, "fit")
                .unwrap_err()
                .code,
            "invalid_social_dimensions"
        );
        assert_eq!(
            validate_dimensions(-2, 720, 1280, 720, "fit")
                .unwrap_err()
                .code,
            "invalid_social_dimensions"
        );
        assert_eq!(
            validate_dimensions(1279, 720, 1280, 720, "fit")
                .unwrap_err()
                .code,
            "invalid_social_dimensions"
        );
        assert_eq!(
            validate_dimensions(1080, 1920, 1920, 1080, "fill").unwrap(),
            (1080, 1920)
        );
        assert_eq!(
            validate_dimensions(1080, 1920, 1920, 1080, "stretch").unwrap(),
            (1080, 1920)
        );
        assert_eq!(
            validate_frame_rate(0.0).unwrap_err().code,
            "invalid_social_frame_rate"
        );
        assert_eq!(
            validate_frame_rate(-1.0).unwrap_err().code,
            "invalid_social_frame_rate"
        );
        assert_eq!(
            validate_frame_rate(120.001).unwrap_err().code,
            "invalid_social_frame_rate"
        );
        assert_eq!(resolve_fit_background("black", None).unwrap(), "0x000000");
        assert_eq!(resolve_fit_background("white", None).unwrap(), "0xFFFFFF");
        assert_eq!(
            resolve_fit_background("custom", Some("#336699")).unwrap(),
            "0x336699"
        );
        assert_eq!(
            resolve_fit_background("custom", Some("not-a-color"))
                .unwrap_err()
                .code,
            "invalid_social_background"
        );
        assert_eq!(resolve_fit_background("blurred", None).unwrap(), "blurred");
        assert_eq!(resolve_quality("smaller").unwrap().qp, 30);
        assert_eq!(resolve_quality("balanced").unwrap().qp, 23);
        assert_eq!(resolve_quality("high").unwrap().qp, 20);
        assert_eq!(resolve_quality("maximum").unwrap().qp, 17);
        assert_eq!(
            resolve_quality("unknown").unwrap_err().code,
            "invalid_social_quality"
        );
        assert_eq!(
            resolve_crop_position("center", 0.0, 0.0).unwrap(),
            CropPosition {
                x_percent: 50.0,
                y_percent: 50.0
            }
        );
        assert_eq!(
            resolve_crop_position("top", 0.0, 0.0).unwrap(),
            CropPosition {
                x_percent: 50.0,
                y_percent: 0.0
            }
        );
        assert_eq!(
            resolve_crop_position("bottom", 0.0, 0.0).unwrap(),
            CropPosition {
                x_percent: 50.0,
                y_percent: 100.0
            }
        );
        assert_eq!(
            resolve_crop_position("left", 0.0, 0.0).unwrap(),
            CropPosition {
                x_percent: 0.0,
                y_percent: 50.0
            }
        );
        assert_eq!(
            resolve_crop_position("right", 0.0, 0.0).unwrap(),
            CropPosition {
                x_percent: 100.0,
                y_percent: 50.0
            }
        );
        assert_eq!(
            resolve_crop_position("custom", -25.0, 175.0).unwrap(),
            CropPosition {
                x_percent: 0.0,
                y_percent: 100.0
            }
        );
        assert_eq!(
            resolve_crop_position("custom", f64::NAN, 50.0)
                .unwrap_err()
                .code,
            "invalid_crop_position"
        );
    }

    fn corner_rgb(path: &Path) -> [u8; 3] {
        let output = Command::new(tool("ffmpeg"))
            .args(["-v", "error", "-i"])
            .arg(path)
            .args([
                "-frames:v",
                "1",
                "-vf",
                "crop=2:2:0:0",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "pipe:1",
            ])
            .env("PATH", path_with_runtime_libraries(&libraries()))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout[..3].try_into().unwrap()
    }

    fn close_color(actual: [u8; 3], expected: [u8; 3]) -> bool {
        actual
            .into_iter()
            .zip(expected)
            .all(|(left, right)| left.abs_diff(right) <= 12)
    }

    #[test]
    fn really_exports_fit_landscape_portrait_square_and_vertical_with_real_backgrounds() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("outputs");
        fs::create_dir(&destination).unwrap();
        for (name, width, height, target_width, target_height, background, custom, expected) in [
            (
                "horizontal-vertical.mp4",
                640,
                360,
                1080,
                1920,
                "black",
                None,
                [0, 0, 0],
            ),
            (
                "vertical-landscape.mp4",
                360,
                640,
                1920,
                1080,
                "white",
                None,
                [255, 255, 255],
            ),
            (
                "horizontal-square.mp4",
                640,
                360,
                1080,
                1080,
                "custom",
                Some("#336699"),
                [51, 102, 153],
            ),
            (
                "vertical-portrait.mp4",
                360,
                640,
                1080,
                1350,
                "black",
                None,
                [0, 0, 0],
            ),
        ] {
            let input = temp.path().join(name);
            create_video(&input, width, height, true, 0.7);
            let original = fs::read(&input).unwrap();
            let mut export_request = request(
                &input,
                &destination,
                "social.mp4",
                target_width,
                target_height,
            );
            export_request.background = background.into();
            export_request.custom_color = custom.map(str::to_owned);
            let result = export_with_tools(
                export_request,
                &tool("ffmpeg"),
                &tool("ffprobe"),
                &libraries(),
            )
            .unwrap();
            assert_eq!(
                (result.width, result.height),
                (target_width as u64, target_height as u64)
            );
            assert_eq!(result.video_codec, "H264");
            assert_eq!(result.audio_codec.as_deref(), Some("AAC"));
            assert!(result.final_size > 0);
            assert!(close_color(
                corner_rgb(Path::new(&result.output_path)),
                expected
            ));
            assert_eq!(fs::read(&input).unwrap(), original);
        }
        assert!(destination.join("social.mp4").is_file());
        assert!(destination.join("social-2.mp4").is_file());
        assert!(destination.join("social-3.mp4").is_file());
        assert!(destination.join("social-4.mp4").is_file());
        assert_eq!(
            fs::read_dir(&destination)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
                .count(),
            0
        );
    }

    fn audio_video_durations(path: &Path) -> (f64, f64) {
        let output = Command::new(tool("ffprobe"))
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type,duration",
                "-of",
                "json",
            ])
            .arg(path)
            .env("PATH", path_with_runtime_libraries(&libraries()))
            .output()
            .unwrap();
        assert!(output.status.success());
        let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let streams = document["streams"].as_array().unwrap();
        let duration = |kind: &str| {
            streams
                .iter()
                .find(|stream| stream["codec_type"].as_str() == Some(kind))
                .and_then(|stream| stream["duration"].as_str())
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap()
        };
        (duration("video"), duration("audio"))
    }

    fn first_frame_hash(path: &Path) -> String {
        let output = Command::new(tool("ffmpeg"))
            .args(["-v", "error", "-i"])
            .arg(path)
            .args([
                "-map",
                "0:v:0",
                "-frames:v",
                "1",
                "-f",
                "hash",
                "-hash",
                "md5",
                "pipe:1",
            ])
            .env("PATH", path_with_runtime_libraries(&libraries()))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn probe_document(path: &Path) -> serde_json::Value {
        let output = Command::new(tool("ffprobe"))
            .args([
                "-v",
                "error",
                "-show_streams",
                "-show_format",
                "-of",
                "json",
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
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn assert_probe_streams(
        path: &Path,
        width: u64,
        height: u64,
        frame_rate: f64,
        expect_audio: bool,
        expected_title: Option<&str>,
    ) {
        let document = probe_document(path);
        let streams = document["streams"].as_array().unwrap();
        let video = streams
            .iter()
            .find(|stream| stream["codec_type"] == "video")
            .unwrap();
        assert_eq!(video["codec_name"], "h264");
        assert_eq!(video["width"].as_u64(), Some(width));
        assert_eq!(video["height"].as_u64(), Some(height));
        let rate = video["avg_frame_rate"].as_str().unwrap();
        let (numerator, denominator) = rate.split_once('/').unwrap();
        let actual_rate = numerator.parse::<f64>().unwrap() / denominator.parse::<f64>().unwrap();
        assert!(
            (actual_rate - frame_rate).abs() <= 0.02,
            "{actual_rate} != {frame_rate}"
        );
        let audio = streams
            .iter()
            .find(|stream| stream["codec_type"] == "audio");
        assert_eq!(audio.is_some(), expect_audio);
        if let Some(audio) = audio {
            assert_eq!(audio["codec_name"], "aac");
        }
        assert_eq!(document["format"]["tags"]["title"].as_str(), expected_title);
    }

    #[test]
    fn really_exports_fill_all_positions_ratios_and_exact_ffprobe_dimensions() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("fill-outputs");
        fs::create_dir(&destination).unwrap();
        let horizontal = temp.path().join("fill-horizontal.mp4");
        let vertical = temp.path().join("fill-vertical.mp4");
        create_video(&horizontal, 640, 360, true, 0.7);
        create_video(&vertical, 360, 640, true, 0.7);
        let cases = [
            (&horizontal, "center", 50.0, 50.0, 1080, 1920),
            (&horizontal, "left", 50.0, 50.0, 1080, 1920),
            (&horizontal, "right", 50.0, 50.0, 1080, 1920),
            (&horizontal, "custom", 25.0, 75.0, 1080, 1920),
            (&vertical, "top", 50.0, 50.0, 1920, 1080),
            (&vertical, "bottom", 50.0, 50.0, 1920, 1080),
            (&horizontal, "center", 50.0, 50.0, 1080, 1080),
            (&horizontal, "custom", 65.0, 35.0, 1080, 1350),
        ];
        let mut hashes = Vec::new();
        for (index, (input, position, x, y, width, height)) in cases.into_iter().enumerate() {
            let mut export_request = request(
                input,
                &destination,
                &format!("fill-{index}.mp4"),
                width,
                height,
            );
            export_request.method = "fill".into();
            export_request.crop_position = position.into();
            export_request.crop_x = x;
            export_request.crop_y = y;
            let result = export_with_tools(
                export_request,
                &tool("ffmpeg"),
                &tool("ffprobe"),
                &libraries(),
            )
            .unwrap();
            assert_eq!((result.width, result.height), (width as u64, height as u64));
            assert_eq!(result.width % 2, 0);
            assert_eq!(result.height % 2, 0);
            assert_eq!(result.video_codec, "H264");
            hashes.push(first_frame_hash(Path::new(&result.output_path)));
        }
        assert_ne!(hashes[0], hashes[1]);
        assert_ne!(hashes[0], hashes[2]);
        assert_ne!(hashes[1], hashes[2]);
        assert_ne!(hashes[0], hashes[3]);
        assert_ne!(hashes[4], hashes[5]);
    }

    #[test]
    fn really_exports_confirmed_stretch_frame_rates_audio_and_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("stretch-outputs");
        fs::create_dir(&destination).unwrap();
        let with_audio = temp.path().join("stretch-with-audio.mp4");
        let without_audio = temp.path().join("stretch-without-audio.mp4");
        create_video_with_options(
            &with_audio,
            640,
            360,
            true,
            0.8,
            12.0,
            Some("FileNova Stretch Source"),
        );
        create_video_with_options(&without_audio, 360, 640, false, 0.8, 18.0, None);

        let mut unconfirmed = request(&with_audio, &destination, "unconfirmed.mp4", 1080, 1920);
        unconfirmed.method = "stretch".into();
        assert_eq!(
            export_with_tools(unconfirmed, &tool("ffmpeg"), &tool("ffprobe"), &libraries())
                .unwrap_err()
                .code,
            "stretch_confirmation_required"
        );

        let cases = [
            (
                &with_audio,
                "original.mp4",
                1080,
                1920,
                true,
                12.0,
                true,
                true,
                Some("FileNova Stretch Source"),
            ),
            (
                &with_audio,
                "15-fps.mp4",
                1080,
                1080,
                false,
                15.0,
                true,
                false,
                None,
            ),
            (
                &with_audio,
                "24-fps-no-audio.mp4",
                1080,
                1920,
                false,
                24.0,
                false,
                true,
                Some("FileNova Stretch Source"),
            ),
            (
                &with_audio,
                "30-fps.mp4",
                1080,
                1080,
                false,
                30.0,
                true,
                false,
                None,
            ),
            (
                &without_audio,
                "source-without-audio.mp4",
                1080,
                1920,
                true,
                18.0,
                true,
                false,
                None,
            ),
        ];
        for (
            input,
            name,
            width,
            height,
            preserve_rate,
            rate,
            preserve_audio,
            preserve_metadata,
            title,
        ) in cases
        {
            let mut export_request = request(input, &destination, name, width, height);
            export_request.method = "stretch".into();
            export_request.stretch_confirmed = true;
            export_request.preserve_frame_rate = preserve_rate;
            export_request.frames_per_second = rate;
            export_request.preserve_audio = preserve_audio;
            export_request.preserve_metadata = preserve_metadata;
            let result = export_with_tools(
                export_request,
                &tool("ffmpeg"),
                &tool("ffprobe"),
                &libraries(),
            )
            .unwrap();
            assert_probe_streams(
                Path::new(&result.output_path),
                width as u64,
                height as u64,
                rate,
                preserve_audio && input == &with_audio,
                title,
            );
        }
    }

    #[test]
    fn really_compares_all_openh264_quality_profiles_progress_and_unique_names() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("quality-outputs");
        fs::create_dir(&destination).unwrap();
        let input = temp.path().join("quality-source.mp4");
        create_video_with_options(&input, 640, 360, false, 2.0, 30.0, None);
        let original = fs::read(&input).unwrap();
        let mut sizes = Vec::new();
        for (index, quality) in ["smaller", "balanced", "high", "maximum"]
            .into_iter()
            .enumerate()
        {
            let mut export_request = request(&input, &destination, "video-social.mp4", 640, 360);
            export_request.quality = quality.into();
            export_request.preserve_audio = false;
            let state = SocialExportState::default();
            let mut progress = Vec::new();
            let result = export_with_tools_state(
                export_request,
                &tool("ffmpeg"),
                &tool("ffprobe"),
                &libraries(),
                &state,
                |event| progress.push(event.percent),
            )
            .unwrap();
            let expected_name = if index == 0 {
                "video-social.mp4".to_owned()
            } else {
                format!("video-social-{}.mp4", index + 1)
            };
            assert_eq!(result.output_name, expected_name);
            assert_eq!(result.video_codec, "H264");
            assert_eq!(result.audio_codec, None);
            assert_eq!((result.width, result.height), (640, 360));
            assert!((result.frames_per_second - 30.0).abs() <= 0.02);
            assert!(progress.iter().any(|percent| *percent > 0.0));
            assert_eq!(progress.last().copied(), Some(100.0));
            assert!(progress.windows(2).all(|pair| pair[0] <= pair[1]));
            sizes.push((quality, result.final_size));
        }
        assert!(
            sizes.windows(2).all(|pair| pair[0].1 < pair[1].1),
            "{sizes:?}"
        );
        println!("Social Media OpenH264 quality sizes: {sizes:?}");
        assert_eq!(fs::read(&input).unwrap(), original);
        assert_eq!(
            fs::read_dir(&destination)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
                .count(),
            0
        );
    }

    #[test]
    fn cancellation_kills_only_social_ffmpeg_and_removes_temporary_output() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("cancel-output");
        fs::create_dir(&destination).unwrap();
        let input = temp.path().join("cancel-source.mp4");
        create_video(&input, 640, 360, false, 20.0);
        let original = fs::read(&input).unwrap();
        let mut export_request = request(&input, &destination, "cancelled-social.mp4", 1080, 1920);
        export_request.quality = "maximum".into();
        export_request.preserve_audio = false;
        let state = SocialExportState::default();
        let worker_state = state.clone();
        let ffmpeg = tool("ffmpeg");
        let ffprobe = tool("ffprobe");
        let libraries = libraries();
        let worker = thread::spawn(move || {
            export_with_tools_state(
                export_request,
                &ffmpeg,
                &ffprobe,
                &libraries,
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
        assert!(killed, "Social Media FFmpeg did not become active");
        assert_eq!(
            worker.join().unwrap().unwrap_err().code,
            "social_export_cancelled"
        );
        assert_eq!(fs::read(&input).unwrap(), original);
        assert!(!destination.join("cancelled-social.mp4").exists());
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    }

    #[test]
    fn really_exports_blurred_tiktok_instagram_youtube_square_short_and_long_videos() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("blurred-outputs");
        fs::create_dir(&destination).unwrap();
        let cases = [
            ("tiktok-short.mp4", 640, 360, 1080, 1920, 0.7),
            ("instagram-reel-long.mp4", 640, 360, 1080, 1920, 2.2),
            ("youtube-standard.mp4", 360, 640, 1920, 1080, 0.9),
            ("square.mp4", 640, 360, 1080, 1080, 1.1),
        ];
        let mut first_input = None;
        let mut first_blurred = None;
        for (name, width, height, target_width, target_height, duration) in cases {
            let input = temp.path().join(name);
            create_video(&input, width, height, true, duration);
            let mut export_request = request(
                &input,
                &destination,
                &format!("{name}-blurred.mp4"),
                target_width,
                target_height,
            );
            export_request.background = "blurred".into();
            let result = export_with_tools(
                export_request,
                &tool("ffmpeg"),
                &tool("ffprobe"),
                &libraries(),
            )
            .unwrap();
            assert_eq!(
                (result.width, result.height),
                (target_width as u64, target_height as u64)
            );
            assert_eq!(result.video_codec, "H264");
            assert_eq!(result.audio_codec.as_deref(), Some("AAC"));
            let (video_duration, audio_duration) =
                audio_video_durations(Path::new(&result.output_path));
            assert!((video_duration - audio_duration).abs() <= 0.12);
            assert!((video_duration - duration).abs() <= 0.15);
            assert!(!close_color(
                corner_rgb(Path::new(&result.output_path)),
                [0, 0, 0]
            ));
            if first_input.is_none() {
                first_input = Some(input);
                first_blurred = Some(result);
            }
        }

        let input = first_input.unwrap();
        let black = export_with_tools(
            request(&input, &destination, "comparison-black.mp4", 1080, 1920),
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
        )
        .unwrap();
        let blurred = first_blurred.unwrap();
        assert!(blurred.final_size > black.final_size);
        assert!(close_color(
            corner_rgb(Path::new(&black.output_path)),
            [0, 0, 0]
        ));
        assert_ne!(
            corner_rgb(Path::new(&blurred.output_path)),
            corner_rgb(Path::new(&black.output_path))
        );
    }
}
