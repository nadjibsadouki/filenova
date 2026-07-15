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
    video_conversion_plan::{build_video_conversion_plan, VideoConversionPlanRequest},
    video_tools::{
        find_sidecar, inspect_video_with_tools, path_with_runtime_libraries, runtime_library_dir,
    },
};

#[derive(Clone, Default)]
pub struct VideoConversionState {
    inner: Arc<VideoConversionInner>,
}

#[derive(Default)]
struct VideoConversionInner {
    child: Mutex<Option<Child>>,
    cancel_requested: AtomicBool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertVideoRequest {
    path: String,
    destination_dir: Option<String>,
    output_format: String,
    video_codec: String,
    audio_codec: String,
    preserve_resolution: bool,
    preserve_frame_rate: bool,
    preserve_metadata: bool,
    quality: u8,
    custom_width: Option<u64>,
    custom_height: Option<u64>,
    custom_frame_rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoConversionProgress {
    path: String,
    name: String,
    processed_seconds: f64,
    duration_seconds: f64,
    percent: f64,
    phase: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertVideoResult {
    original_format: String,
    final_format: String,
    original_video_codec: String,
    final_video_codec: String,
    original_audio_codec: Option<String>,
    final_audio_codec: Option<String>,
    remux_without_reencoding: bool,
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
pub struct VideoConversionError {
    code: &'static str,
    message: String,
}

impl VideoConversionError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[tauri::command]
pub async fn convert_video(
    app: AppHandle,
    window: Window,
    state: State<'_, VideoConversionState>,
    request: ConvertVideoRequest,
) -> Result<ConvertVideoResult, VideoConversionError> {
    let ffmpeg = find_sidecar(&app, "ffmpeg")
        .map_err(|error| VideoConversionError::new("ffmpeg_unavailable", error))?;
    let ffprobe = find_sidecar(&app, "ffprobe")
        .map_err(|error| VideoConversionError::new("ffprobe_unavailable", error))?;
    let library_dir = runtime_library_dir(&app);
    let conversion_state = state.inner().clone();
    let progress_window = window.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        convert_video_with_tools(
            request,
            &ffmpeg,
            &ffprobe,
            &library_dir,
            &conversion_state,
            |progress| {
                let _ = progress_window.emit("video-conversion-progress", progress);
            },
        )
    })
    .await
    .map_err(|error| VideoConversionError::new("conversion_failed", error.to_string()))??;
    app.asset_protocol_scope()
        .allow_file(Path::new(&result.output_path))
        .map_err(|error| VideoConversionError::new("output_invalid", error.to_string()))?;
    Ok(result)
}

#[tauri::command]
pub fn cancel_video_conversion(
    state: State<'_, VideoConversionState>,
) -> Result<bool, VideoConversionError> {
    let mut active =
        state.inner.child.lock().map_err(|_| {
            VideoConversionError::new("conversion_failed", "Process lock poisoned.")
        })?;
    let Some(child) = active.as_mut() else {
        return Ok(false);
    };
    state.inner.cancel_requested.store(true, Ordering::SeqCst);
    child
        .kill()
        .map_err(|error| VideoConversionError::new("cancel_failed", error.to_string()))?;
    Ok(true)
}

fn convert_video_with_tools<F>(
    request: ConvertVideoRequest,
    ffmpeg: &Path,
    ffprobe: &Path,
    library_dir: &Path,
    state: &VideoConversionState,
    mut report: F,
) -> Result<ConvertVideoResult, VideoConversionError>
where
    F: FnMut(VideoConversionProgress),
{
    let started_at = Instant::now();
    let output_format = request.output_format.to_ascii_lowercase();
    if !matches!(output_format.as_str(), "mp4" | "mov") {
        return Err(VideoConversionError::new(
            "conversion_format_not_implemented",
            "Real conversion currently supports MP4 and MOV only.",
        ));
    }
    let input_path = PathBuf::from(&request.path);
    let input = inspect_video_with_tools(&input_path, ffprobe, ffmpeg, library_dir)
        .map_err(|error| VideoConversionError::new(error.code, error.message))?;
    if input.duration_seconds <= 0.0 {
        return Err(VideoConversionError::new(
            "invalid_video",
            "The source duration is missing or invalid.",
        ));
    }

    validate_custom_options(&request)?;
    let plan_request = VideoConversionPlanRequest {
        input_video_codec: input.video_codec.clone(),
        input_audio_codec: input.audio_codec.clone(),
        output_format: output_format.clone(),
        video_codec: request.video_codec.clone(),
        audio_codec: request.audio_codec.clone(),
        preserve_resolution: request.preserve_resolution,
        preserve_frame_rate: request.preserve_frame_rate,
    };
    let plan = build_video_conversion_plan(&plan_request);
    if !plan.valid {
        return Err(VideoConversionError::new(
            "incompatible_conversion_plan",
            plan.reason
                .unwrap_or_else(|| "The selected streams are incompatible.".into()),
        ));
    }
    if plan.planned_video_codec == "SVT-AV1" {
        return Err(VideoConversionError::new(
            "conversion_codec_not_implemented",
            "MP4 and MOV conversion currently supports Copy or OpenH264 video only.",
        ));
    }

    let selected_resolution = if request.preserve_resolution {
        None
    } else {
        Some(calculate_output_resolution(
            input.width,
            input.height,
            request.custom_width,
            request.custom_height,
        )?)
    };
    let selected_frame_rate = if request.preserve_frame_rate {
        None
    } else {
        request.custom_frame_rate
    };
    let destination = destination_directory(&input_path, request.destination_dir.as_deref())?;
    let stem = input_path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("video");
    let base_name = format!("{stem}-converted");
    let temporary = temporary_output_path(&destination, &base_name, &output_format);
    verify_destination_writable(&temporary)?;
    let mut cleanup = TemporaryOutput(Some(temporary.clone()));
    let args = conversion_args(
        &request,
        &plan,
        &input_path,
        &temporary,
        selected_resolution,
        selected_frame_rate,
    );
    let phase = if plan.remux_without_reencoding {
        "remuxing"
    } else {
        "converting"
    };
    report(VideoConversionProgress {
        path: request.path.clone(),
        name: input.name.clone(),
        processed_seconds: 0.0,
        duration_seconds: input.duration_seconds,
        percent: 0.0,
        phase: phase.into(),
    });
    run_ffmpeg(
        ffmpeg,
        library_dir,
        state,
        &args,
        &request.path,
        &input.name,
        input.duration_seconds,
        phase,
        &mut report,
    )?;
    let verified = validate_output(&temporary, ffprobe, ffmpeg, library_dir)?;
    let final_output = unique_output_path(&destination, &base_name, &output_format);
    fs::rename(&temporary, &final_output).map_err(|error| {
        VideoConversionError::new("destination_inaccessible", error.to_string())
    })?;
    cleanup.0 = None;
    let final_size = fs::metadata(&final_output)
        .map_err(|error| VideoConversionError::new("output_invalid", error.to_string()))?
        .len();
    report(VideoConversionProgress {
        path: request.path,
        name: input.name,
        processed_seconds: input.duration_seconds,
        duration_seconds: input.duration_seconds,
        percent: 100.0,
        phase: phase.into(),
    });
    Ok(ConvertVideoResult {
        original_format: input.format,
        final_format: output_format.to_ascii_uppercase(),
        original_video_codec: input.video_codec,
        final_video_codec: verified.video_codec,
        original_audio_codec: input.audio_codec,
        final_audio_codec: verified.audio_codec,
        remux_without_reencoding: plan.remux_without_reencoding,
        width: verified.width,
        height: verified.height,
        frames_per_second: verified.frames_per_second,
        original_size: input.size,
        final_size,
        processing_duration_seconds: started_at.elapsed().as_secs_f64(),
        output_name: final_output
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("video-converted")
            .to_owned(),
        output_path: final_output.to_string_lossy().into_owned(),
    })
}

fn validate_custom_options(request: &ConvertVideoRequest) -> Result<(), VideoConversionError> {
    if !request.preserve_resolution
        && request.custom_width.is_none()
        && request.custom_height.is_none()
    {
        return Err(VideoConversionError::new(
            "invalid_custom_resolution",
            "Enter a custom width, height, or both.",
        ));
    }
    for dimension in [request.custom_width, request.custom_height]
        .into_iter()
        .flatten()
    {
        if dimension < 2 {
            return Err(VideoConversionError::new(
                "invalid_custom_resolution",
                "Custom dimensions must be at least 2 pixels.",
            ));
        }
    }
    if !request.preserve_frame_rate
        && !request
            .custom_frame_rate
            .is_some_and(|value| value.is_finite() && value > 0.0 && value <= 240.0)
    {
        return Err(VideoConversionError::new(
            "invalid_custom_frame_rate",
            "Custom frame rate must be greater than 0 and no higher than 240 fps.",
        ));
    }
    Ok(())
}

fn calculate_output_resolution(
    input_width: u64,
    input_height: u64,
    requested_width: Option<u64>,
    requested_height: Option<u64>,
) -> Result<(u64, u64), VideoConversionError> {
    if input_width == 0 || input_height == 0 {
        return Err(VideoConversionError::new(
            "invalid_video",
            "Source resolution is invalid.",
        ));
    }
    let width_scale = requested_width.map(|value| value as f64 / input_width as f64);
    let height_scale = requested_height.map(|value| value as f64 / input_height as f64);
    let scale = match (width_scale, height_scale) {
        (Some(width), Some(height)) => width.min(height),
        (Some(width), None) => width,
        (None, Some(height)) => height,
        (None, None) => 1.0,
    }
    .min(1.0);
    let even = |value: f64| ((value.floor() as u64).max(2) / 2) * 2;
    Ok((
        even(input_width as f64 * scale),
        even(input_height as f64 * scale),
    ))
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

fn conversion_args(
    request: &ConvertVideoRequest,
    plan: &crate::video_conversion_plan::VideoConversionPlan,
    input: &Path,
    output: &Path,
    resolution: Option<(u64, u64)>,
    frame_rate: Option<f64>,
) -> Vec<OsString> {
    let video_copy = plan.planned_video_codec.starts_with("Copy");
    let audio_copy = plan
        .planned_audio_codec
        .as_deref()
        .is_some_and(|codec| codec.starts_with("Copy"));
    let no_audio = plan.planned_audio_codec.is_none();
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
    if no_audio {
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
    if video_copy && audio_copy {
        args.extend(["-c".into(), "copy".into()]);
    } else {
        if video_copy {
            args.extend(["-c:v".into(), "copy".into()]);
        } else {
            if let Some((width, height)) = resolution {
                args.extend(["-vf".into(), format!("scale={width}:{height}").into()]);
            }
            if let Some(fps) = frame_rate {
                args.extend(["-r".into(), format!("{fps:.6}").into()]);
            }
            let quality = openh264_quality_value(request.quality);
            args.extend([
                "-c:v".into(),
                "libopenh264".into(),
                "-rc_mode".into(),
                "quality".into(),
                "-qmin".into(),
                quality.to_string().into(),
                "-qmax".into(),
                quality.to_string().into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
                "-profile:v".into(),
                "main".into(),
            ]);
        }
        if !no_audio {
            if audio_copy {
                args.extend(["-c:a".into(), "copy".into()]);
            } else {
                args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "160k".into()]);
            }
        }
    }
    if request.output_format.eq_ignore_ascii_case("mp4") {
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
    state: &VideoConversionState,
    args: &[OsString],
    input_path: &str,
    input_name: &str,
    duration: f64,
    phase: &str,
    report: &mut F,
) -> Result<(), VideoConversionError>
where
    F: FnMut(VideoConversionProgress),
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
        .map_err(|error| VideoConversionError::new("ffmpeg_unavailable", error.to_string()))?;
    let stdout = child.stdout.take().ok_or_else(|| {
        VideoConversionError::new("conversion_failed", "FFmpeg progress is unavailable.")
    })?;
    let mut stderr = child.stderr.take().ok_or_else(|| {
        VideoConversionError::new("conversion_failed", "FFmpeg error output is unavailable.")
    })?;
    {
        let mut active = state.inner.child.lock().map_err(|_| {
            VideoConversionError::new("conversion_failed", "Process lock poisoned.")
        })?;
        if active.is_some() {
            let _ = child.kill();
            return Err(VideoConversionError::new(
                "conversion_busy",
                "Another video conversion is already running.",
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
                report(VideoConversionProgress {
                    path: input_path.to_owned(),
                    name: input_name.to_owned(),
                    processed_seconds: processed,
                    duration_seconds: duration,
                    percent: (processed / duration * 100.0).clamp(0.0, 99.9),
                    phase: phase.to_owned(),
                });
            }
        }
    }
    let status = state
        .inner
        .child
        .lock()
        .map_err(|_| VideoConversionError::new("conversion_failed", "Process lock poisoned."))?
        .take()
        .ok_or_else(|| VideoConversionError::new("conversion_failed", "FFmpeg disappeared."))?
        .wait()
        .map_err(|error| VideoConversionError::new("conversion_failed", error.to_string()))?;
    let stderr = stderr_reader.join().unwrap_or_default();
    let cancelled = state.inner.cancel_requested.swap(false, Ordering::SeqCst);
    if cancelled {
        return Err(VideoConversionError::new(
            "conversion_cancelled",
            "Video conversion was cancelled.",
        ));
    }
    if !status.success() {
        return Err(classify_ffmpeg_error(&stderr));
    }
    Ok(())
}

fn validate_output(
    path: &Path,
    ffprobe: &Path,
    ffmpeg: &Path,
    library_dir: &Path,
) -> Result<crate::video_tools::VideoInfo, VideoConversionError> {
    let size = fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if size == 0 {
        return Err(VideoConversionError::new(
            "output_empty",
            "FFmpeg created an empty output.",
        ));
    }
    inspect_video_with_tools(path, ffprobe, ffmpeg, library_dir)
        .map_err(|error| VideoConversionError::new("output_invalid", error.message))
}

fn destination_directory(
    input: &Path,
    selected: Option<&str>,
) -> Result<PathBuf, VideoConversionError> {
    let destination = selected
        .map(PathBuf::from)
        .or_else(|| input.parent().map(|parent| parent.join("FileNova")))
        .ok_or_else(|| VideoConversionError::new("destination_inaccessible", "No destination."))?;
    fs::create_dir_all(&destination).map_err(|error| {
        VideoConversionError::new("destination_inaccessible", error.to_string())
    })?;
    Ok(destination)
}

fn unique_output_path(directory: &Path, base: &str, extension: &str) -> PathBuf {
    let first = directory.join(format!("{base}.{extension}"));
    if !first.exists() {
        return first;
    }
    for index in 2_u64.. {
        let candidate = directory.join(format!("{base}-{index}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

fn temporary_output_path(directory: &Path, base: &str, extension: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    directory.join(format!(".{base}.{nonce}.tmp.{extension}"))
}

fn verify_destination_writable(path: &Path) -> Result<(), VideoConversionError> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            VideoConversionError::new("destination_inaccessible", error.to_string())
        })?;
    fs::remove_file(path)
        .map_err(|error| VideoConversionError::new("destination_inaccessible", error.to_string()))
}

fn classify_ffmpeg_error(stderr: &str) -> VideoConversionError {
    let lower = stderr.to_ascii_lowercase();
    let code = if lower.contains("no space left") || lower.contains("disk full") {
        "disk_full"
    } else if lower.contains("permission denied") || lower.contains("access is denied") {
        "destination_inaccessible"
    } else if lower.contains("could not find tag")
        || lower.contains("not currently supported in container")
        || lower.contains("incompatible")
    {
        "incompatible_conversion_plan"
    } else {
        "conversion_failed"
    };
    VideoConversionError::new(code, stderr.trim())
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

    fn bundled_tool(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!("{name}-{TARGET_TRIPLE}.exe"))
    }

    fn library_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/ffmpeg")
    }

    fn request(path: &Path, destination: &Path, format: &str) -> ConvertVideoRequest {
        ConvertVideoRequest {
            path: path.to_string_lossy().into_owned(),
            destination_dir: Some(destination.to_string_lossy().into_owned()),
            output_format: format.into(),
            video_codec: "auto".into(),
            audio_codec: "auto".into(),
            preserve_resolution: true,
            preserve_frame_rate: true,
            preserve_metadata: false,
            quality: 75,
            custom_width: None,
            custom_height: None,
            custom_frame_rate: None,
        }
    }

    fn create_source(path: &Path, format: &str, duration: u64, size: &str) {
        let (video_codec, audio_codec) = match format {
            "avi" => ("mpeg4", "mp3"),
            _ => ("libopenh264", "aac"),
        };
        let mut command = Command::new(bundled_tool("ffmpeg"));
        command
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                &format!("testsrc2=size={size}:rate=30"),
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=700:sample_rate=44100",
                "-t",
                &duration.to_string(),
                "-c:v",
                video_codec,
            ])
            .args(if video_codec == "libopenh264" {
                vec!["-rc_mode", "quality", "-qmin", "24", "-qmax", "24"]
            } else {
                Vec::new()
            })
            .args(["-c:a", audio_codec, "-shortest"])
            .arg(path)
            .env("PATH", path_with_runtime_libraries(&library_dir()));
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn quality_mapping_uses_openh264_supported_quantizer_controls() {
        assert_eq!(openh264_quality_value(1), 35);
        assert_eq!(openh264_quality_value(50), 26);
        assert_eq!(openh264_quality_value(70), 23);
        assert_eq!(openh264_quality_value(85), 20);
        assert_eq!(openh264_quality_value(100), 17);
    }

    #[test]
    fn custom_resolution_preserves_ratio_even_dimensions_and_never_upscales() {
        assert_eq!(
            calculate_output_resolution(1920, 1080, Some(1280), None).unwrap(),
            (1280, 720)
        );
        assert_eq!(
            calculate_output_resolution(1920, 1080, None, Some(480)).unwrap(),
            (852, 480)
        );
        assert_eq!(
            calculate_output_resolution(1920, 1080, Some(1000), Some(1000)).unwrap(),
            (1000, 562)
        );
        assert_eq!(
            calculate_output_resolution(640, 360, Some(1920), None).unwrap(),
            (640, 360)
        );
    }

    #[test]
    fn args_use_copy_or_openh264_without_x264_crf_passes_or_presets() {
        let path = Path::new("C:\\input.mp4");
        let output = Path::new("C:\\output.mp4");
        let req = request(path, Path::new("C:\\"), "mp4");
        let plan = build_video_conversion_plan(&VideoConversionPlanRequest {
            input_video_codec: "H264".into(),
            input_audio_codec: Some("AAC".into()),
            output_format: "mp4".into(),
            video_codec: "copy".into(),
            audio_codec: "copy".into(),
            preserve_resolution: true,
            preserve_frame_rate: true,
        });
        let copy = conversion_args(&req, &plan, path, output, None, None)
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(copy.contains("-c copy"));
        assert!(copy.contains("-movflags +faststart"));

        let mut encode_req = req;
        encode_req.video_codec = "openh264".into();
        encode_req.audio_codec = "aac".into();
        let plan = build_video_conversion_plan(&VideoConversionPlanRequest {
            input_video_codec: "MPEG4".into(),
            input_audio_codec: Some("MP3".into()),
            output_format: "mp4".into(),
            video_codec: "openh264".into(),
            audio_codec: "aac".into(),
            preserve_resolution: false,
            preserve_frame_rate: false,
        });
        let encode = conversion_args(
            &encode_req,
            &plan,
            path,
            output,
            Some((160, 90)),
            Some(15.0),
        )
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
        assert!(encode.contains("-c:v libopenh264 -rc_mode quality"));
        assert!(encode.contains("-pix_fmt yuv420p"));
        assert!(encode.contains("-c:a aac -b:a 160k"));
        assert!(encode.contains("-vf scale=160:90 -r 15.000000"));
        for forbidden in ["libx264", "-crf", "preset", "-pass", "passlog"] {
            assert!(!encode.contains(forbidden));
        }
    }

    #[test]
    fn really_remuxes_mp4_to_mp4_and_mov_with_copy_and_unique_names() {
        let temp = tempfile::tempdir().unwrap();
        let output_dir = temp.path().join("outputs");
        fs::create_dir(&output_dir).unwrap();
        let source = temp.path().join("video.mp4");
        create_source(&source, "mp4", 1, "320x180");
        let original = fs::read(&source).unwrap();
        let state = VideoConversionState::default();

        let first = convert_video_with_tools(
            request(&source, &output_dir, "mp4"),
            &bundled_tool("ffmpeg"),
            &bundled_tool("ffprobe"),
            &library_dir(),
            &state,
            |_| {},
        )
        .unwrap();
        let duplicate = convert_video_with_tools(
            request(&source, &output_dir, "mp4"),
            &bundled_tool("ffmpeg"),
            &bundled_tool("ffprobe"),
            &library_dir(),
            &state,
            |_| {},
        )
        .unwrap();
        let mov = convert_video_with_tools(
            request(&source, &output_dir, "mov"),
            &bundled_tool("ffmpeg"),
            &bundled_tool("ffprobe"),
            &library_dir(),
            &state,
            |_| {},
        )
        .unwrap();

        assert!(first.remux_without_reencoding && mov.remux_without_reencoding);
        assert_eq!(first.output_name, "video-converted.mp4");
        assert_eq!(duplicate.output_name, "video-converted-2.mp4");
        assert_eq!(mov.output_name, "video-converted.mov");
        assert_eq!(
            (
                first.final_video_codec.as_str(),
                first.final_audio_codec.as_deref()
            ),
            ("H264", Some("AAC"))
        );
        assert_eq!(fs::read(&source).unwrap(), original);
    }

    #[test]
    fn really_converts_avi_and_mov_to_mp4_with_openh264_aac_custom_geometry_and_no_audio() {
        let temp = tempfile::tempdir().unwrap();
        let output_dir = temp.path().join("outputs");
        fs::create_dir(&output_dir).unwrap();
        let avi = temp.path().join("clip.avi");
        let mov = temp.path().join("movie.mov");
        create_source(&avi, "avi", 1, "320x180");
        create_source(&mov, "mov", 1, "320x180");
        let avi_original = fs::read(&avi).unwrap();
        let mov_original = fs::read(&mov).unwrap();
        let state = VideoConversionState::default();

        let mut avi_request = request(&avi, &output_dir, "mp4");
        avi_request.video_codec = "openh264".into();
        avi_request.audio_codec = "aac".into();
        avi_request.preserve_resolution = false;
        avi_request.custom_width = Some(160);
        avi_request.preserve_frame_rate = false;
        avi_request.custom_frame_rate = Some(15.0);
        let avi_result = convert_video_with_tools(
            avi_request,
            &bundled_tool("ffmpeg"),
            &bundled_tool("ffprobe"),
            &library_dir(),
            &state,
            |_| {},
        )
        .unwrap();
        assert!(!avi_result.remux_without_reencoding);
        assert_eq!((avi_result.width, avi_result.height), (160, 90));
        assert!((avi_result.frames_per_second - 15.0).abs() < 0.05);
        assert_eq!(avi_result.final_video_codec, "H264");
        assert_eq!(avi_result.final_audio_codec.as_deref(), Some("AAC"));

        let mov_result = convert_video_with_tools(
            request(&mov, &output_dir, "mp4"),
            &bundled_tool("ffmpeg"),
            &bundled_tool("ffprobe"),
            &library_dir(),
            &state,
            |_| {},
        )
        .unwrap();
        assert!(mov_result.remux_without_reencoding);

        let mut silent_request = request(&avi, &output_dir, "mov");
        silent_request.video_codec = "openh264".into();
        silent_request.audio_codec = "noAudio".into();
        let silent = convert_video_with_tools(
            silent_request,
            &bundled_tool("ffmpeg"),
            &bundled_tool("ffprobe"),
            &library_dir(),
            &state,
            |_| {},
        )
        .unwrap();
        assert_eq!(silent.final_audio_codec, None);
        assert_eq!(fs::read(&avi).unwrap(), avi_original);
        assert_eq!(fs::read(&mov).unwrap(), mov_original);
        assert!(fs::read_dir(&output_dir).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp.")));
    }

    #[test]
    fn cancellation_stops_only_conversion_ffmpeg_and_cleans_temporary_file() {
        let temp = tempfile::tempdir().unwrap();
        let output_dir = temp.path().join("outputs");
        fs::create_dir(&output_dir).unwrap();
        let source = temp.path().join("long.avi");
        create_source(&source, "avi", 8, "1280x720");
        let original = fs::read(&source).unwrap();
        let state = VideoConversionState::default();
        let worker_state = state.clone();
        let worker_source = source.clone();
        let worker_output = output_dir.clone();
        let worker = thread::spawn(move || {
            let mut conversion = request(&worker_source, &worker_output, "mp4");
            conversion.video_codec = "openh264".into();
            conversion.audio_codec = "aac".into();
            convert_video_with_tools(
                conversion,
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
            thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(killed);
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error.code, "conversion_cancelled");
        assert_eq!(fs::read(&source).unwrap(), original);
        assert_eq!(fs::read_dir(&output_dir).unwrap().count(), 0);
    }
}
