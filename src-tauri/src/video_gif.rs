use std::{
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, Window};

use crate::{
    video_gif_plan::{
        calculate_dimensions, resolve_requested_width, validate_frame_rate, validate_selection,
    },
    video_tools::{
        find_sidecar, inspect_video_with_tools, path_with_runtime_libraries, runtime_library_dir,
    },
};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Default)]
pub struct VideoGifState {
    inner: Arc<VideoGifInner>,
}

#[derive(Default)]
struct VideoGifInner {
    child: Mutex<Option<Child>>,
    running: AtomicBool,
    cancel_requested: AtomicBool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum GifQuality {
    Small,
    Balanced,
    High,
    Maximum,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum GifDithering {
    None,
    Bayer,
    FloydSteinberg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ColorProfile {
    max_colors: u16,
    stats_mode: &'static str,
    bayer_scale: u8,
    difference_rectangle: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGifRequest {
    path: String,
    destination_dir: Option<String>,
    output_name: String,
    start_seconds: f64,
    end_seconds: f64,
    dimension: String,
    custom_width: Option<f64>,
    keep_aspect_ratio: bool,
    frames_per_second: f64,
    quality: GifQuality,
    optimize_colors: bool,
    dithering: GifDithering,
    play_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGifResult {
    output_path: String,
    output_name: String,
    start_seconds: f64,
    end_seconds: f64,
    duration_seconds: f64,
    width: u64,
    height: u64,
    frames_per_second: f64,
    final_size: u64,
    processing_duration_seconds: f64,
    profile: GifQuality,
    optimize_colors: bool,
    dithering: GifDithering,
    play_count: Option<u32>,
    frame_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GifProgress {
    phase: &'static str,
    phase_percent: f64,
    overall_percent: f64,
    processed_seconds: f64,
    duration_seconds: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateGifError {
    code: &'static str,
    message: String,
}

impl CreateGifError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[tauri::command]
pub async fn create_video_gif(
    app: AppHandle,
    window: Window,
    state: State<'_, VideoGifState>,
    request: CreateGifRequest,
) -> Result<CreateGifResult, CreateGifError> {
    let ffmpeg = find_sidecar(&app, "ffmpeg")
        .map_err(|error| CreateGifError::new("ffmpeg_unavailable", error))?;
    let ffprobe = find_sidecar(&app, "ffprobe")
        .map_err(|error| CreateGifError::new("ffprobe_unavailable", error))?;
    let library_dir = runtime_library_dir(&app);
    if state.inner.running.swap(true, Ordering::SeqCst) {
        return Err(CreateGifError::new(
            "gif_busy",
            "Another GIF creation is already running.",
        ));
    }
    state.inner.cancel_requested.store(false, Ordering::SeqCst);
    let process_state = state.inner.clone();
    let progress_window = window.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let _running = RunningGuard(process_state.clone());
        let mut report = |progress| {
            let _ = progress_window.emit("video-gif-progress", progress);
        };
        create_gif_with_tools(
            request,
            &ffmpeg,
            &ffprobe,
            &library_dir,
            &process_state,
            &mut report,
        )
    })
    .await
    .map_err(|error| CreateGifError::new("gif_creation_failed", error.to_string()))??;
    app.asset_protocol_scope()
        .allow_file(Path::new(&result.output_path))
        .map_err(|error| CreateGifError::new("output_invalid", error.to_string()))?;
    Ok(result)
}

#[tauri::command]
pub fn cancel_video_gif(state: State<'_, VideoGifState>) -> Result<bool, CreateGifError> {
    request_cancellation(&state.inner)
}

fn create_gif_with_tools(
    request: CreateGifRequest,
    ffmpeg: &Path,
    ffprobe: &Path,
    library_dir: &Path,
    state: &VideoGifInner,
    report: &mut dyn FnMut(GifProgress),
) -> Result<CreateGifResult, CreateGifError> {
    let started = Instant::now();
    let input_path = PathBuf::from(&request.path)
        .canonicalize()
        .map_err(|error| CreateGifError::new("video_not_found", error.to_string()))?;
    let input = inspect_video_with_tools(&input_path, ffprobe, ffmpeg, library_dir)
        .map_err(|error| CreateGifError::new(error.code, error.message))?;
    validate_selection(
        request.start_seconds,
        request.end_seconds,
        input.duration_seconds,
    )
    .map_err(|code| CreateGifError::new(code, "The selected GIF time range is invalid."))?;
    validate_frame_rate(request.frames_per_second).map_err(|code| {
        CreateGifError::new(code, "The GIF frame rate must be between 1 and 120 fps.")
    })?;
    let requested_width =
        resolve_requested_width(input.width, &request.dimension, request.custom_width)
            .ok_or_else(|| CreateGifError::new("invalid_dimensions", "Invalid GIF width."))?;
    let dimensions = calculate_dimensions(
        input.width,
        input.height,
        requested_width,
        request.keep_aspect_ratio,
    )
    .ok_or_else(|| CreateGifError::new("invalid_dimensions", "Invalid GIF dimensions."))?;
    let output_stem = output_stem(&request.output_name)?;
    let destination = destination_directory(&input_path, request.destination_dir.as_deref())?;
    let final_output = unique_output_path(&destination, &output_stem);
    let nonce = unique_nonce();
    let palette = destination.join(format!(".{output_stem}.{nonce}.palette.png"));
    let temporary_gif = destination.join(format!(".{output_stem}.{nonce}.tmp.gif"));
    verify_destination_writable(&temporary_gif)?;
    let mut cleanup = TemporaryFiles(vec![palette.clone(), temporary_gif.clone()]);
    let duration = request.end_seconds - request.start_seconds;
    let selection = SelectionArguments::new(request.start_seconds, duration);
    let filter = processing_filter(
        request.frames_per_second,
        dimensions.width,
        dimensions.height,
    );
    let profile = color_profile(request.quality);
    let loop_value = gif_loop_value(request.play_count)?;
    let palette_args = palette_arguments(
        &input_path,
        &palette,
        &selection,
        &filter,
        profile,
        request.optimize_colors,
    );
    run_ffmpeg(
        ffmpeg,
        library_dir,
        state,
        &palette_args,
        "palette_generation_failed",
        "palette",
        duration,
        0.0,
        40.0,
        report,
    )?;
    if !palette.is_file() {
        return Err(CreateGifError::new(
            "palette_generation_failed",
            "FFmpeg did not create the temporary palette.",
        ));
    }
    let gif_args = gif_arguments(
        &input_path,
        &palette,
        &temporary_gif,
        &selection,
        &filter,
        request.dithering,
        profile,
        &loop_value,
    );
    run_ffmpeg(
        ffmpeg,
        library_dir,
        state,
        &gif_args,
        "gif_creation_failed",
        "gif",
        duration,
        40.0,
        60.0,
        report,
    )?;
    let final_size = fs::metadata(&temporary_gif)
        .map_err(|error| CreateGifError::new("output_invalid", error.to_string()))?
        .len();
    let mut signature = [0_u8; 4];
    let valid_signature = fs::File::open(&temporary_gif)
        .and_then(|mut file| file.read_exact(&mut signature))
        .map(|_| signature == *b"GIF8")
        .unwrap_or(false);
    if final_size == 0 || !valid_signature {
        return Err(CreateGifError::new(
            "output_invalid",
            "FFmpeg did not create a valid GIF file.",
        ));
    }
    let frame_count = validate_gif_output(
        &temporary_gif,
        ffprobe,
        library_dir,
        dimensions.width,
        dimensions.height,
        duration,
        request.frames_per_second,
    )?;
    if state.cancel_requested.load(Ordering::SeqCst) {
        return Err(CreateGifError::new(
            "gif_cancelled",
            "GIF creation was cancelled.",
        ));
    }
    fs::rename(&temporary_gif, &final_output)
        .map_err(|error| CreateGifError::new("destination_inaccessible", error.to_string()))?;
    cleanup.0.retain(|path| path != &temporary_gif);

    Ok(CreateGifResult {
        output_path: final_output.to_string_lossy().into_owned(),
        output_name: final_output
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("output.gif")
            .to_owned(),
        start_seconds: request.start_seconds,
        end_seconds: request.end_seconds,
        duration_seconds: duration,
        width: dimensions.width,
        height: dimensions.height,
        frames_per_second: request.frames_per_second,
        final_size,
        processing_duration_seconds: started.elapsed().as_secs_f64(),
        profile: request.quality,
        optimize_colors: request.optimize_colors,
        dithering: request.dithering,
        play_count: request.play_count,
        frame_count,
    })
}

#[derive(Debug, Clone)]
struct SelectionArguments {
    start: String,
    duration: String,
}

impl SelectionArguments {
    fn new(start: f64, duration: f64) -> Self {
        Self {
            start: format!("{start:.6}"),
            duration: format!("{duration:.6}"),
        }
    }
}

fn processing_filter(frames_per_second: f64, width: u64, height: u64) -> String {
    format!("fps={frames_per_second:.6},scale={width}:{height}:flags=lanczos")
}

fn color_profile(quality: GifQuality) -> ColorProfile {
    match quality {
        GifQuality::Small => ColorProfile {
            max_colors: 64,
            stats_mode: "diff",
            bayer_scale: 4,
            difference_rectangle: true,
        },
        GifQuality::Balanced => ColorProfile {
            max_colors: 128,
            stats_mode: "diff",
            bayer_scale: 3,
            difference_rectangle: true,
        },
        GifQuality::High => ColorProfile {
            max_colors: 192,
            stats_mode: "full",
            bayer_scale: 2,
            difference_rectangle: false,
        },
        GifQuality::Maximum => ColorProfile {
            max_colors: 256,
            stats_mode: "full",
            bayer_scale: 1,
            difference_rectangle: false,
        },
    }
}

fn paletteuse_options(dithering: GifDithering, profile: ColorProfile) -> String {
    let mut options = match dithering {
        GifDithering::None => "dither=none".to_owned(),
        GifDithering::Bayer => {
            format!("dither=bayer:bayer_scale={}", profile.bayer_scale)
        }
        GifDithering::FloydSteinberg => "dither=floyd_steinberg".to_owned(),
    };
    if profile.difference_rectangle {
        options.push_str(":diff_mode=rectangle");
    }
    options
}

/// Maps the number of complete plays requested by the UI to FFmpeg's GIF muxer value.
///
/// `-loop 0` writes a NETSCAPE loop count of 0, which means infinite repetition.
/// `-loop -1` writes no loop extension, so the animation is read exactly once.
/// A positive GIF loop count is the number of *additional repetitions after the
/// first play*. Therefore two total plays map to `-loop 1`, three to `-loop 2`, etc.
fn gif_loop_value(play_count: Option<u32>) -> Result<String, CreateGifError> {
    match play_count {
        None => Ok("0".into()),
        Some(0) => Err(CreateGifError::new(
            "invalid_loop",
            "The GIF play count must be positive.",
        )),
        Some(1) => Ok("-1".into()),
        Some(plays @ 2..=65_536) => Ok((plays - 1).to_string()),
        Some(_) => Err(CreateGifError::new(
            "invalid_loop",
            "The GIF play count exceeds the format limit.",
        )),
    }
}

fn palette_arguments(
    input: &Path,
    palette: &Path,
    selection: &SelectionArguments,
    filter: &str,
    profile: ColorProfile,
    optimize_colors: bool,
) -> Vec<OsString> {
    let palette_filter = if optimize_colors {
        format!(
            "palettegen=max_colors={}:stats_mode={}:reserve_transparent=false",
            profile.max_colors, profile.stats_mode
        )
    } else {
        // The non-optimized path deliberately keeps only the palette calculated for
        // the latest frame. It is valid but less representative than a full clip histogram.
        "palettegen=max_colors=128:stats_mode=single:reserve_transparent=false".into()
    };
    let filter_graph =
        format!("{filter},split[progress][palette_input];[palette_input]{palette_filter}[palette]");
    let mut arguments = [
        "-v",
        "error",
        "-progress",
        "pipe:1",
        "-nostats",
        "-stats_period",
        "0.1",
        "-y",
        "-ss",
        &selection.start,
        "-t",
        &selection.duration,
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .chain([input.as_os_str().to_owned()])
    .chain([
        "-filter_complex".into(),
        filter_graph.into(),
        "-map".into(),
        "[palette]".into(),
    ])
    .collect::<Vec<_>>();
    if optimize_colors {
        arguments.extend([OsString::from("-frames:v"), OsString::from("1")]);
    } else {
        arguments.extend([OsString::from("-update"), OsString::from("1")]);
    }
    arguments.push(palette.as_os_str().to_owned());
    arguments.extend([
        OsString::from("-map"),
        OsString::from("[progress]"),
        OsString::from("-f"),
        OsString::from("null"),
        OsString::from("NUL"),
    ]);
    arguments
}

fn gif_arguments(
    input: &Path,
    palette: &Path,
    output: &Path,
    selection: &SelectionArguments,
    filter: &str,
    dithering: GifDithering,
    profile: ColorProfile,
    loop_value: &str,
) -> Vec<OsString> {
    [
        "-v",
        "error",
        "-progress",
        "pipe:1",
        "-nostats",
        "-stats_period",
        "0.1",
        "-y",
        "-ss",
        &selection.start,
        "-t",
        &selection.duration,
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .chain([
        input.as_os_str().to_owned(),
        "-i".into(),
        palette.as_os_str().to_owned(),
    ])
    .chain([
        "-lavfi".into(),
        format!(
            "{filter}[frames];[frames][1:v]paletteuse={}",
            paletteuse_options(dithering, profile)
        )
        .into(),
        "-an".into(),
        "-loop".into(),
        loop_value.into(),
        output.as_os_str().to_owned(),
    ])
    .collect()
}

fn run_ffmpeg(
    ffmpeg: &Path,
    library_dir: &Path,
    state: &VideoGifInner,
    arguments: &[OsString],
    code: &'static str,
    phase: &'static str,
    duration_seconds: f64,
    overall_start: f64,
    overall_weight: f64,
    report: &mut dyn FnMut(GifProgress),
) -> Result<(), CreateGifError> {
    if state.cancel_requested.load(Ordering::SeqCst) {
        return Err(CreateGifError::new(
            "gif_cancelled",
            "GIF creation was cancelled.",
        ));
    }
    let mut child = Command::new(ffmpeg)
        .args(arguments)
        .env("PATH", path_with_runtime_libraries(library_dir))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| CreateGifError::new("ffmpeg_unavailable", error.to_string()))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    {
        let mut active = state
            .child
            .lock()
            .map_err(|_| CreateGifError::new("gif_creation_failed", "Process lock poisoned."))?;
        if active.is_some() {
            let _ = child.kill();
            return Err(CreateGifError::new(
                "gif_busy",
                "Another GIF process is already running.",
            ));
        }
        *active = Some(child);
        if state.cancel_requested.load(Ordering::SeqCst) {
            let _ = active.as_mut().unwrap().kill();
        }
    }
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_end(&mut bytes);
        }
        bytes
    });
    let (progress_sender, progress_receiver) = mpsc::channel();
    let stdout_reader = thread::spawn(move || {
        if let Some(stdout) = stdout {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = progress_sender.send(line);
            }
        }
    });
    report(GifProgress {
        phase,
        phase_percent: 0.0,
        overall_percent: overall_start,
        processed_seconds: 0.0,
        duration_seconds,
    });
    let status = loop {
        while let Ok(line) = progress_receiver.try_recv() {
            report_progress_line(
                &line,
                phase,
                duration_seconds,
                overall_start,
                overall_weight,
                report,
            );
        }
        let status = {
            let mut active = state.child.lock().map_err(|_| {
                CreateGifError::new("gif_creation_failed", "Process lock poisoned.")
            })?;
            let child = active.as_mut().ok_or_else(|| {
                CreateGifError::new("gif_creation_failed", "Active FFmpeg process was lost.")
            })?;
            match child.try_wait() {
                Ok(Some(status)) => {
                    active.take();
                    Some(status)
                }
                Ok(None) => None,
                Err(error) => {
                    active.take();
                    return Err(CreateGifError::new(
                        "gif_creation_failed",
                        error.to_string(),
                    ));
                }
            }
        };
        if let Some(status) = status {
            break status;
        }
        thread::sleep(Duration::from_millis(15));
    };
    let _ = stdout_reader.join();
    while let Ok(line) = progress_receiver.try_recv() {
        report_progress_line(
            &line,
            phase,
            duration_seconds,
            overall_start,
            overall_weight,
            report,
        );
    }
    let stderr = stderr_reader.join().unwrap_or_default();
    if state.cancel_requested.load(Ordering::SeqCst) {
        return Err(CreateGifError::new(
            "gif_cancelled",
            "GIF creation was cancelled.",
        ));
    }
    if status.success() {
        report(GifProgress {
            phase,
            phase_percent: 100.0,
            overall_percent: overall_start + overall_weight,
            processed_seconds: duration_seconds,
            duration_seconds,
        });
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&stderr).trim().to_owned();
    let lower = detail.to_ascii_lowercase();
    let classified = if lower.contains("no space left") || lower.contains("disk full") {
        "disk_full"
    } else if lower.contains("permission denied") || lower.contains("access is denied") {
        "destination_inaccessible"
    } else {
        code
    };
    Err(CreateGifError::new(classified, detail))
}

fn report_progress_line(
    line: &str,
    phase: &'static str,
    duration_seconds: f64,
    overall_start: f64,
    overall_weight: f64,
    report: &mut dyn FnMut(GifProgress),
) {
    let Some(value) = line
        .strip_prefix("out_time_us=")
        .or_else(|| line.strip_prefix("out_time_ms="))
    else {
        return;
    };
    let Some(microseconds) = value.parse::<f64>().ok() else {
        return;
    };
    let processed_seconds = (microseconds / 1_000_000.0).clamp(0.0, duration_seconds);
    let phase_percent = if duration_seconds > 0.0 {
        processed_seconds / duration_seconds * 100.0
    } else {
        0.0
    };
    report(GifProgress {
        phase,
        phase_percent,
        overall_percent: overall_start + overall_weight * phase_percent / 100.0,
        processed_seconds,
        duration_seconds,
    });
}

fn validate_gif_output(
    path: &Path,
    ffprobe: &Path,
    library_dir: &Path,
    expected_width: u64,
    expected_height: u64,
    duration: f64,
    frames_per_second: f64,
) -> Result<u64, CreateGifError> {
    if !path.is_file() {
        return Err(CreateGifError::new(
            "output_invalid",
            "The temporary GIF does not exist.",
        ));
    }
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-count_frames",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,nb_read_frames",
            "-of",
            "json",
        ])
        .arg(path)
        .env("PATH", path_with_runtime_libraries(library_dir))
        .output()
        .map_err(|error| CreateGifError::new("ffprobe_unavailable", error.to_string()))?;
    if !output.status.success() {
        return Err(CreateGifError::new(
            "output_invalid",
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| CreateGifError::new("output_invalid", error.to_string()))?;
    let stream = metadata["streams"]
        .as_array()
        .and_then(|streams| streams.first())
        .ok_or_else(|| CreateGifError::new("output_invalid", "The GIF has no video stream."))?;
    let width = stream["width"].as_u64().unwrap_or_default();
    let height = stream["height"].as_u64().unwrap_or_default();
    let frame_count = stream["nb_read_frames"]
        .as_str()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    if width != expected_width || height != expected_height {
        return Err(CreateGifError::new(
            "output_invalid",
            "The generated GIF dimensions do not match the request.",
        ));
    }
    if frame_count == 0 || (duration * frames_per_second > 1.0 && frame_count < 2) {
        return Err(CreateGifError::new(
            "output_invalid",
            "The generated GIF does not contain the expected animation frames.",
        ));
    }
    Ok(frame_count)
}

fn request_cancellation(state: &Arc<VideoGifInner>) -> Result<bool, CreateGifError> {
    if !state.running.load(Ordering::SeqCst) {
        return Ok(false);
    }
    state.cancel_requested.store(true, Ordering::SeqCst);
    let mut active = state
        .child
        .lock()
        .map_err(|_| CreateGifError::new("gif_creation_failed", "Process lock poisoned."))?;
    if let Some(child) = active.as_mut() {
        child
            .kill()
            .map_err(|error| CreateGifError::new("cancel_failed", error.to_string()))?;
    }
    Ok(true)
}

struct RunningGuard(Arc<VideoGifInner>);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::SeqCst);
        self.0.cancel_requested.store(false, Ordering::SeqCst);
        if let Ok(mut active) = self.0.child.lock() {
            if let Some(child) = active.as_mut() {
                let _ = child.kill();
            }
            active.take();
        }
    }
}

fn output_stem(name: &str) -> Result<String, CreateGifError> {
    let trimmed = name.trim();
    let path = Path::new(trimmed);
    if trimmed.is_empty()
        || path.components().count() != 1
        || path.file_name() != Some(path.as_os_str())
    {
        return Err(CreateGifError::new(
            "invalid_output_name",
            "The GIF output name is invalid.",
        ));
    }
    let stem = trimmed
        .strip_suffix(".gif")
        .or_else(|| trimmed.strip_suffix(".GIF"))
        .unwrap_or(trimmed)
        .trim();
    if stem.is_empty() {
        return Err(CreateGifError::new(
            "invalid_output_name",
            "The GIF output name is invalid.",
        ));
    }
    Ok(stem.to_owned())
}

fn destination_directory(input: &Path, selected: Option<&str>) -> Result<PathBuf, CreateGifError> {
    let destination = selected
        .map(PathBuf::from)
        .or_else(|| input.parent().map(|parent| parent.join("FileNova")))
        .ok_or_else(|| CreateGifError::new("destination_inaccessible", "No destination."))?;
    fs::create_dir_all(&destination)
        .map_err(|error| CreateGifError::new("destination_inaccessible", error.to_string()))?;
    Ok(destination)
}

fn unique_output_path(directory: &Path, stem: &str) -> PathBuf {
    let first = directory.join(format!("{stem}.gif"));
    if !first.exists() {
        return first;
    }
    for index in 2_u64.. {
        let candidate = directory.join(format!("{stem}-{index}.gif"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

fn unique_nonce() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let sequence = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    format!("{}-{nanos}-{sequence}", std::process::id())
}

fn verify_destination_writable(path: &Path) -> Result<(), CreateGifError> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| CreateGifError::new("destination_inaccessible", error.to_string()))?;
    fs::remove_file(path)
        .map_err(|error| CreateGifError::new("destination_inaccessible", error.to_string()))
}

struct TemporaryFiles(Vec<PathBuf>);

impl Drop for TemporaryFiles {
    fn drop(&mut self) {
        for path in &self.0 {
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

    fn create_video(path: &Path, width: u64, height: u64) {
        let output = Command::new(tool("ffmpeg"))
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                &format!("testsrc2=size={width}x{height}:rate=30"),
                "-t",
                "2",
                "-c:v",
                "libopenh264",
                "-rc_mode",
                "quality",
                "-qmin",
                "24",
                "-qmax",
                "24",
                "-an",
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

    fn request(
        input: &Path,
        destination: &Path,
        name: &str,
        dimension: &str,
        custom_width: Option<f64>,
        frames_per_second: f64,
    ) -> CreateGifRequest {
        CreateGifRequest {
            path: input.to_string_lossy().into_owned(),
            destination_dir: Some(destination.to_string_lossy().into_owned()),
            output_name: name.into(),
            start_seconds: 0.25,
            end_seconds: 1.25,
            dimension: dimension.into(),
            custom_width,
            keep_aspect_ratio: true,
            frames_per_second,
            quality: GifQuality::Balanced,
            optimize_colors: true,
            dithering: GifDithering::Bayer,
            play_count: None,
        }
    }

    fn probe_gif(path: &Path) -> (u64, u64, f64, f64) {
        let probe = Command::new(tool("ffprobe"))
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=width,height,r_frame_rate:format=duration",
                "-of",
                "json",
            ])
            .arg(path)
            .env("PATH", path_with_runtime_libraries(&libraries()))
            .output()
            .unwrap();
        assert!(
            probe.status.success(),
            "{}",
            String::from_utf8_lossy(&probe.stderr)
        );
        let metadata: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        let stream = &metadata["streams"][0];
        let rate = stream["r_frame_rate"].as_str().unwrap();
        let (numerator, denominator) = rate.split_once('/').unwrap();
        let frames_per_second =
            numerator.parse::<f64>().unwrap() / denominator.parse::<f64>().unwrap();
        let duration = metadata["format"]["duration"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap();
        (
            stream["width"].as_u64().unwrap(),
            stream["height"].as_u64().unwrap(),
            frames_per_second,
            duration,
        )
    }

    fn measure_psnr(source: &Path, gif: &Path, width: u64, height: u64, fps: f64) -> f64 {
        let filter = format!(
            "[0:v]fps={fps:.6},scale={width}:{height}:flags=lanczos,setpts=PTS-STARTPTS[reference];[1:v]setpts=PTS-STARTPTS[gif];[reference][gif]psnr"
        );
        let output = Command::new(tool("ffmpeg"))
            .args(["-v", "info", "-ss", "0.25", "-t", "1", "-i"])
            .arg(source)
            .arg("-i")
            .arg(gif)
            .args(["-filter_complex", &filter, "-t", "1", "-f", "null", "NUL"])
            .env("PATH", path_with_runtime_libraries(&libraries()))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        let average = stderr.rsplit("average:").next().unwrap();
        average.split_whitespace().next().unwrap().parse().unwrap()
    }

    fn netscape_loop_count(path: &Path) -> Option<u16> {
        let bytes = fs::read(path).unwrap();
        for marker in [b"NETSCAPE2.0".as_slice(), b"ANIMEXTS1.0".as_slice()] {
            if let Some(position) = bytes
                .windows(marker.len())
                .position(|window| window == marker)
            {
                let data = position + marker.len();
                if bytes.get(data..data + 4).is_some() && bytes[data] == 3 && bytes[data + 1] == 1 {
                    return Some(u16::from_le_bytes([bytes[data + 2], bytes[data + 3]]));
                }
            }
        }
        None
    }

    #[test]
    fn maps_total_plays_to_additional_gif_repetitions() {
        assert_eq!(gif_loop_value(None).unwrap(), "0");
        assert_eq!(gif_loop_value(Some(1)).unwrap(), "-1");
        assert_eq!(gif_loop_value(Some(2)).unwrap(), "1");
        assert_eq!(gif_loop_value(Some(3)).unwrap(), "2");
        assert_eq!(gif_loop_value(Some(65_536)).unwrap(), "65535");
        assert_eq!(gif_loop_value(Some(0)).unwrap_err().code, "invalid_loop");
        assert_eq!(
            gif_loop_value(Some(65_537)).unwrap_err().code,
            "invalid_loop"
        );
    }

    #[test]
    fn quality_profiles_and_dithering_map_only_to_supported_palette_options() {
        let profiles = [
            (GifQuality::Small, 64, "diff", 4, true),
            (GifQuality::Balanced, 128, "diff", 3, true),
            (GifQuality::High, 192, "full", 2, false),
            (GifQuality::Maximum, 256, "full", 1, false),
        ];
        for (quality, colors, stats_mode, bayer_scale, difference_rectangle) in profiles {
            let profile = color_profile(quality);
            assert_eq!(profile.max_colors, colors);
            assert_eq!(profile.stats_mode, stats_mode);
            assert_eq!(profile.bayer_scale, bayer_scale);
            assert_eq!(profile.difference_rectangle, difference_rectangle);
        }
        let balanced = color_profile(GifQuality::Balanced);
        assert_eq!(
            paletteuse_options(GifDithering::None, balanced),
            "dither=none:diff_mode=rectangle"
        );
        assert_eq!(
            paletteuse_options(GifDithering::Bayer, balanced),
            "dither=bayer:bayer_scale=3:diff_mode=rectangle"
        );
        assert_eq!(
            paletteuse_options(GifDithering::FloydSteinberg, balanced),
            "dither=floyd_steinberg:diff_mode=rectangle"
        );
    }

    #[test]
    fn palette_and_gif_use_identical_selection_fps_and_scale_filters() {
        let selection = SelectionArguments::new(1.125, 2.75);
        let filter = processing_filter(15.0, 480, 270);
        let profile = color_profile(GifQuality::Balanced);
        let palette = palette_arguments(
            Path::new("input.mp4"),
            Path::new("palette.png"),
            &selection,
            &filter,
            profile,
            true,
        );
        let gif = gif_arguments(
            Path::new("input.mp4"),
            Path::new("palette.png"),
            Path::new("output.gif"),
            &selection,
            &filter,
            GifDithering::Bayer,
            profile,
            "0",
        );
        for arguments in [&palette, &gif] {
            let values = arguments
                .iter()
                .map(|value| value.to_string_lossy())
                .collect::<Vec<_>>();
            let selection = values.windows(4).find(|values| values[0] == "-ss").unwrap();
            assert_eq!(selection, ["-ss", "1.125000", "-t", "2.750000"]);
        }
        let palette_filter = palette
            .windows(2)
            .find(|pair| pair[0] == "-filter_complex")
            .unwrap()[1]
            .to_string_lossy();
        let gif_filter =
            gif.windows(2).find(|pair| pair[0] == "-lavfi").unwrap()[1].to_string_lossy();
        assert!(palette_filter.starts_with(&filter));
        assert!(gif_filter.starts_with(&filter));
        assert_eq!(filter, "fps=15.000000,scale=480:270:flags=lanczos");
    }

    #[test]
    fn reports_real_palette_phase_gif_phase_and_monotonic_overall_progress() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("progress.mp4");
        let destination = temp.path().join("outputs");
        create_video(&input, 640, 360);
        let mut progress = Vec::new();
        create_gif_with_tools(
            request(&input, &destination, "progress.gif", "360", None, 15.0),
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &VideoGifInner::default(),
            &mut |event| progress.push(event),
        )
        .unwrap();
        assert!(progress.iter().any(|event| {
            event.phase == "palette"
                && event.phase_percent == 100.0
                && event.overall_percent == 40.0
                && event.processed_seconds > 0.0
        }));
        assert!(progress.iter().any(|event| {
            event.phase == "gif"
                && event.phase_percent == 100.0
                && event.overall_percent == 100.0
                && event.processed_seconds > 0.0
        }));
        assert!(progress.iter().any(|event| {
            event.phase == "palette" && event.phase_percent > 0.0 && event.phase_percent < 100.0
        }));
        assert!(progress.iter().any(|event| {
            event.phase == "gif" && event.phase_percent > 0.0 && event.phase_percent < 100.0
        }));
        assert!(progress.windows(2).all(|events| {
            events[1].overall_percent + f64::EPSILON >= events[0].overall_percent
        }));
    }

    #[test]
    fn really_creates_horizontal_and_vertical_gifs_at_requested_sizes_and_frame_rates() {
        let temp = tempfile::tempdir().unwrap();
        let horizontal = temp.path().join("horizontal.mp4");
        let vertical = temp.path().join("vertical.mp4");
        let destination = temp.path().join("outputs");
        create_video(&horizontal, 640, 360);
        create_video(&vertical, 360, 640);
        let horizontal_original = fs::read(&horizontal).unwrap();
        let vertical_original = fs::read(&vertical).unwrap();
        let cases = [
            (
                &horizontal,
                "horizontal-360.gif",
                "360",
                None,
                5.0,
                (360, 202),
            ),
            (
                &horizontal,
                "horizontal-480.gif",
                "480",
                None,
                15.0,
                (480, 270),
            ),
            (
                &horizontal,
                "horizontal-custom.gif",
                "custom",
                Some(422.0),
                30.0,
                (422, 238),
            ),
            (
                &vertical,
                "vertical-no-upscale.gif",
                "480",
                None,
                15.0,
                (360, 640),
            ),
        ];
        for (input, name, dimension, custom_width, fps, expected_dimensions) in cases {
            let result = create_gif_with_tools(
                request(input, &destination, name, dimension, custom_width, fps),
                &tool("ffmpeg"),
                &tool("ffprobe"),
                &libraries(),
                &VideoGifInner::default(),
                &mut |_: GifProgress| {},
            )
            .unwrap();
            let gif = fs::read(&result.output_path).unwrap();
            assert!(gif.starts_with(b"GIF89a") || gif.starts_with(b"GIF87a"));
            assert_eq!((result.width, result.height), expected_dimensions);
            assert!(result.width > 0 && result.height > 0);
            assert_eq!(result.width % 2, 0);
            assert_eq!(result.height % 2, 0);
            assert_eq!(result.duration_seconds, 1.0);
            assert_eq!(result.start_seconds, 0.25);
            assert_eq!(result.end_seconds, 1.25);
            assert!(result.final_size > 100);
            let (width, height, actual_fps, actual_duration) =
                probe_gif(Path::new(&result.output_path));
            assert_eq!((width, height), expected_dimensions);
            assert!((actual_fps - fps).abs() < 0.01);
            assert!((actual_duration - 1.0).abs() <= 0.21);
            let source_ratio = if input == &horizontal {
                640.0 / 360.0
            } else {
                360.0 / 640.0
            };
            let output_ratio = width as f64 / height as f64;
            assert!((source_ratio - output_ratio).abs() < 0.01);
        }
        assert_eq!(fs::read(&horizontal).unwrap(), horizontal_original);
        assert_eq!(fs::read(&vertical).unwrap(), vertical_original);
        let leftovers = fs::read_dir(&destination)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn rejects_invalid_frame_rates_widths_and_time_selections_before_encoding() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("source.mp4");
        let destination = temp.path().join("outputs");
        create_video(&input, 640, 360);
        let invalid = [
            (
                request(&input, &destination, "zero-fps.gif", "360", None, 0.0),
                "invalid_frame_rate",
            ),
            (
                request(&input, &destination, "negative-fps.gif", "360", None, -5.0),
                "invalid_frame_rate",
            ),
            (
                request(
                    &input,
                    &destination,
                    "zero-width.gif",
                    "custom",
                    Some(0.0),
                    15.0,
                ),
                "invalid_dimensions",
            ),
            (
                request(
                    &input,
                    &destination,
                    "negative-width.gif",
                    "custom",
                    Some(-20.0),
                    15.0,
                ),
                "invalid_dimensions",
            ),
        ];
        for (request, expected_code) in invalid {
            let error = create_gif_with_tools(
                request,
                &tool("ffmpeg"),
                &tool("ffprobe"),
                &libraries(),
                &VideoGifInner::default(),
                &mut |_: GifProgress| {},
            )
            .unwrap_err();
            assert_eq!(error.code, expected_code);
        }
        let mut invalid_selection = request(
            &input,
            &destination,
            "invalid-selection.gif",
            "360",
            None,
            15.0,
        );
        invalid_selection.start_seconds = 1.5;
        invalid_selection.end_seconds = 1.0;
        let error = create_gif_with_tools(
            invalid_selection,
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &VideoGifInner::default(),
            &mut |_: GifProgress| {},
        )
        .unwrap_err();
        assert_eq!(error.code, "start_after_end");
        assert!(!destination.exists() || fs::read_dir(destination).unwrap().next().is_none());
    }

    #[test]
    fn really_compares_quality_dithering_and_color_optimization_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("quality-source.mp4");
        let destination = temp.path().join("outputs");
        create_video(&input, 640, 360);

        let qualities = [
            (GifQuality::Small, "small.gif"),
            (GifQuality::Balanced, "balanced.gif"),
            (GifQuality::High, "high.gif"),
            (GifQuality::Maximum, "maximum.gif"),
        ];
        let mut quality_comparison = Vec::new();
        for (quality, name) in qualities {
            let mut gif_request = request(&input, &destination, name, "360", None, 15.0);
            gif_request.quality = quality;
            gif_request.dithering = GifDithering::None;
            let result = create_gif_with_tools(
                gif_request,
                &tool("ffmpeg"),
                &tool("ffprobe"),
                &libraries(),
                &VideoGifInner::default(),
                &mut |_: GifProgress| {},
            )
            .unwrap();
            let psnr = measure_psnr(
                &input,
                Path::new(&result.output_path),
                result.width,
                result.height,
                result.frames_per_second,
            );
            eprintln!(
                "quality={quality:?} colors={} size={} psnr={psnr:.3}",
                color_profile(quality).max_colors,
                result.final_size
            );
            quality_comparison.push((quality, result.final_size, psnr));
        }
        let small = quality_comparison.first().unwrap();
        let maximum = quality_comparison.last().unwrap();
        assert!(maximum.1 > small.1);
        assert!(maximum.2 >= small.2);

        let dithering_modes = [
            (GifDithering::None, "dither-none.gif"),
            (GifDithering::Bayer, "dither-bayer.gif"),
            (GifDithering::FloydSteinberg, "dither-floyd.gif"),
        ];
        let mut dither_sizes = Vec::new();
        for (dithering, name) in dithering_modes {
            let mut gif_request = request(&input, &destination, name, "360", None, 15.0);
            gif_request.dithering = dithering;
            let result = create_gif_with_tools(
                gif_request,
                &tool("ffmpeg"),
                &tool("ffprobe"),
                &libraries(),
                &VideoGifInner::default(),
                &mut |_: GifProgress| {},
            )
            .unwrap();
            let psnr = measure_psnr(
                &input,
                Path::new(&result.output_path),
                result.width,
                result.height,
                result.frames_per_second,
            );
            eprintln!(
                "dithering={dithering:?} size={} psnr={psnr:.3}",
                result.final_size
            );
            dither_sizes.push(result.final_size);
        }
        dither_sizes.sort_unstable();
        dither_sizes.dedup();
        assert!(dither_sizes.len() >= 2);

        let mut direct_request = request(
            &input,
            &destination,
            "without-optimization.gif",
            "360",
            None,
            15.0,
        );
        direct_request.optimize_colors = false;
        direct_request.dithering = GifDithering::FloydSteinberg;
        let direct = create_gif_with_tools(
            direct_request,
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &VideoGifInner::default(),
            &mut |_: GifProgress| {},
        )
        .unwrap();
        let direct_probe = probe_gif(Path::new(&direct.output_path));
        let direct_psnr = measure_psnr(
            &input,
            Path::new(&direct.output_path),
            direct.width,
            direct.height,
            direct.frames_per_second,
        );
        eprintln!(
            "optimize_colors=false size={} psnr={direct_psnr:.3}",
            direct.final_size
        );
        assert!(!direct.optimize_colors);
        assert_eq!(direct_probe.0, 360);
        assert_eq!(direct_probe.1, 202);
        assert!((direct_probe.2 - 15.0).abs() < 0.01);
        assert!(direct.final_size > 100);
    }

    #[test]
    fn really_writes_infinite_and_limited_loops_unique_names_and_preserves_source() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("video.mp4");
        let destination = temp.path().join("outputs");
        create_video(&input, 640, 360);
        let original = fs::read(&input).unwrap();
        let cases = [
            (None, Some(0), "video-animation.gif"),
            (Some(1), None, "video-animation-2.gif"),
            (Some(2), Some(1), "video-animation-3.gif"),
            (Some(3), Some(2), "video-animation-4.gif"),
        ];
        for (play_count, expected_loop_count, expected_name) in cases {
            let mut gif_request = request(
                &input,
                &destination,
                "video-animation.gif",
                "360",
                None,
                15.0,
            );
            gif_request.play_count = play_count;
            let result = create_gif_with_tools(
                gif_request,
                &tool("ffmpeg"),
                &tool("ffprobe"),
                &libraries(),
                &VideoGifInner::default(),
                &mut |_: GifProgress| {},
            )
            .unwrap();
            let output = Path::new(&result.output_path);
            assert!(output.is_file());
            assert!(result.final_size > 0);
            assert_eq!(result.output_name, expected_name);
            assert_eq!(result.play_count, play_count);
            assert_eq!(netscape_loop_count(output), expected_loop_count);
            assert_eq!(probe_gif(output).0, 360);
            assert_eq!(probe_gif(output).1, 202);
            assert!(result.frame_count >= 2);
        }
        assert_eq!(fs::read(&input).unwrap(), original);
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 4);
    }

    #[test]
    fn cancellation_during_gif_kills_only_ffmpeg_and_removes_temporary_files() {
        let temp = tempfile::tempdir().unwrap();
        let palette = temp.path().join(".cancel.palette.png");
        let temporary_gif = temp.path().join(".cancel.tmp.gif");
        fs::write(&palette, b"temporary palette").unwrap();
        let state = Arc::new(VideoGifInner::default());
        state.running.store(true, Ordering::SeqCst);
        let worker_state = state.clone();
        let worker_palette = palette.clone();
        let worker_gif = temporary_gif.clone();
        let worker = thread::spawn(move || {
            let _running = RunningGuard(worker_state.clone());
            let _cleanup = TemporaryFiles(vec![worker_palette, worker_gif.clone()]);
            let arguments = vec![
                "-v".into(),
                "error".into(),
                "-y".into(),
                "-re".into(),
                "-f".into(),
                "lavfi".into(),
                "-i".into(),
                "testsrc2=size=1280x720:rate=30".into(),
                "-t".into(),
                "30".into(),
                "-vf".into(),
                "fps=30,scale=720:404:flags=lanczos".into(),
                "-loop".into(),
                "0".into(),
                worker_gif.as_os_str().to_owned(),
            ];
            run_ffmpeg(
                &tool("ffmpeg"),
                &libraries(),
                &worker_state,
                &arguments,
                "gif_creation_failed",
                "gif",
                30.0,
                40.0,
                60.0,
                &mut |_: GifProgress| {},
            )
        });
        let started = Instant::now();
        while state.child.lock().unwrap().is_none() && started.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(state.child.lock().unwrap().is_some());
        assert!(request_cancellation(&state).unwrap());
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error.code, "gif_cancelled");
        assert!(!palette.exists());
        assert!(!temporary_gif.exists());
        assert!(!state.running.load(Ordering::SeqCst));
    }

    #[test]
    fn cancellation_during_palette_generation_removes_palette_and_temporary_gif() {
        let temp = tempfile::tempdir().unwrap();
        let palette = temp.path().join(".palette-cancel.palette.png");
        let temporary_gif = temp.path().join(".palette-cancel.tmp.gif");
        fs::write(&temporary_gif, b"temporary gif").unwrap();
        let state = Arc::new(VideoGifInner::default());
        state.running.store(true, Ordering::SeqCst);
        let worker_state = state.clone();
        let worker_palette = palette.clone();
        let worker_gif = temporary_gif.clone();
        let worker = thread::spawn(move || {
            let _running = RunningGuard(worker_state.clone());
            let _cleanup = TemporaryFiles(vec![worker_palette.clone(), worker_gif]);
            let arguments = vec![
                "-v".into(),
                "error".into(),
                "-progress".into(),
                "pipe:1".into(),
                "-nostats".into(),
                "-y".into(),
                "-re".into(),
                "-f".into(),
                "lavfi".into(),
                "-i".into(),
                "testsrc2=size=1280x720:rate=30".into(),
                "-t".into(),
                "30".into(),
                "-vf".into(),
                "fps=30,scale=720:404:flags=lanczos,palettegen=max_colors=128:stats_mode=diff"
                    .into(),
                "-frames:v".into(),
                "1".into(),
                worker_palette.as_os_str().to_owned(),
            ];
            run_ffmpeg(
                &tool("ffmpeg"),
                &libraries(),
                &worker_state,
                &arguments,
                "palette_generation_failed",
                "palette",
                30.0,
                0.0,
                40.0,
                &mut |_: GifProgress| {},
            )
        });
        let started = Instant::now();
        while state.child.lock().unwrap().is_none() && started.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(state.child.lock().unwrap().is_some());
        assert!(request_cancellation(&state).unwrap());
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error.code, "gif_cancelled");
        assert!(!palette.exists());
        assert!(!temporary_gif.exists());
        assert!(!state.running.load(Ordering::SeqCst));
    }
}
