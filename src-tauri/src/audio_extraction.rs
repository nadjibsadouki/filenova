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
    audio_extraction_plan::{
        build_audio_extraction_plan, detect_capabilities, inspect_audio_source_with_tool,
        AudioExtractionPlan, AudioExtractionPlanRequest,
    },
    video_tools::{find_sidecar, path_with_runtime_libraries, runtime_library_dir},
};

#[derive(Clone, Default)]
pub struct AudioExtractionState {
    inner: Arc<AudioExtractionInner>,
}

#[derive(Default)]
struct AudioExtractionInner {
    child: Mutex<Option<Child>>,
    cancel_requested: AtomicBool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractAudioRequest {
    path: String,
    destination_dir: Option<String>,
    output_format: String,
    quality: String,
    custom_bitrate: Option<u64>,
    sample_rate: Option<u64>,
    channels: Option<u64>,
    preserve_metadata: bool,
    copy_audio_stream: bool,
    selected_audio_track: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioExtractionProgress {
    path: String,
    name: String,
    processed_seconds: f64,
    duration_seconds: f64,
    percent: f64,
    phase: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractAudioResult {
    source_name: String,
    final_format: String,
    final_codec: String,
    final_bitrate: Option<u64>,
    sample_rate: Option<u64>,
    channels: Option<u64>,
    selected_audio_track: usize,
    selected_audio_language: Option<String>,
    extract_without_reencoding: bool,
    final_size: u64,
    processing_duration_seconds: f64,
    output_path: String,
    output_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioExtractionRunError {
    code: &'static str,
    message: String,
}

impl AudioExtractionRunError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[tauri::command]
pub async fn extract_audio(
    app: AppHandle,
    window: Window,
    state: State<'_, AudioExtractionState>,
    request: ExtractAudioRequest,
) -> Result<ExtractAudioResult, AudioExtractionRunError> {
    let ffmpeg = find_sidecar(&app, "ffmpeg")
        .map_err(|error| AudioExtractionRunError::new("ffmpeg_unavailable", error))?;
    let ffprobe = find_sidecar(&app, "ffprobe")
        .map_err(|error| AudioExtractionRunError::new("ffprobe_unavailable", error))?;
    let library_dir = runtime_library_dir(&app);
    let extraction_state = state.inner().clone();
    let progress_window = window.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        extract_audio_with_tools(
            request,
            &ffmpeg,
            &ffprobe,
            &library_dir,
            &extraction_state,
            |progress| {
                let _ = progress_window.emit("audio-extraction-progress", progress);
            },
        )
    })
    .await
    .map_err(|error| AudioExtractionRunError::new("extraction_failed", error.to_string()))??;
    app.asset_protocol_scope()
        .allow_file(Path::new(&result.output_path))
        .map_err(|error| AudioExtractionRunError::new("output_invalid", error.to_string()))?;
    Ok(result)
}

#[tauri::command]
pub fn cancel_audio_extraction(
    state: State<'_, AudioExtractionState>,
) -> Result<bool, AudioExtractionRunError> {
    let mut active =
        state.inner.child.lock().map_err(|_| {
            AudioExtractionRunError::new("extraction_failed", "Process lock poisoned.")
        })?;
    let Some(child) = active.as_mut() else {
        return Ok(false);
    };
    state.inner.cancel_requested.store(true, Ordering::SeqCst);
    child
        .kill()
        .map_err(|error| AudioExtractionRunError::new("cancel_failed", error.to_string()))?;
    Ok(true)
}

fn extract_audio_with_tools<F>(
    request: ExtractAudioRequest,
    ffmpeg: &Path,
    ffprobe: &Path,
    library_dir: &Path,
    state: &AudioExtractionState,
    mut report: F,
) -> Result<ExtractAudioResult, AudioExtractionRunError>
where
    F: FnMut(AudioExtractionProgress),
{
    let started_at = Instant::now();
    let input_path = PathBuf::from(&request.path);
    let input = inspect_audio_source_with_tool(&input_path, ffprobe, library_dir)
        .map_err(|error| AudioExtractionRunError::new(error.code, error.message))?;
    let track = input
        .audio_tracks
        .get(request.selected_audio_track)
        .cloned()
        .ok_or_else(|| {
            AudioExtractionRunError::new(
                if input.audio_tracks.is_empty() {
                    "no_audio_stream"
                } else {
                    "audio_track_not_found"
                },
                "The selected audio track does not exist.",
            )
        })?;
    let output_format = request.output_format.to_ascii_lowercase();
    let capabilities = detect_capabilities(ffmpeg, library_dir)
        .map_err(|error| AudioExtractionRunError::new("ffmpeg_unavailable", error))?;
    let supported = match output_format.as_str() {
        "mp3" => capabilities.mp3,
        "wav" => capabilities.wav,
        "m4a" => capabilities.m4a,
        "flac" => capabilities.flac,
        "ogg" => capabilities.ogg_opus,
        _ => false,
    };
    if !supported {
        return Err(AudioExtractionRunError::new(
            "codec_unsupported",
            "The embedded FFmpeg build does not support this output format.",
        ));
    }
    let plan_request = AudioExtractionPlanRequest {
        input_audio_codec: Some(track.codec.clone()),
        input_audio_bitrate: track.bitrate,
        input_sample_rate: track.sample_rate,
        input_channels: track.channels,
        output_format: output_format.clone(),
        quality: request.quality.clone(),
        custom_bitrate: request.custom_bitrate,
        sample_rate: request.sample_rate,
        channels: request.channels,
        copy_audio_stream: request.copy_audio_stream,
    };
    let plan = build_audio_extraction_plan(&plan_request);
    if !plan.valid {
        return Err(AudioExtractionRunError::new(
            "incompatible_audio_plan",
            plan.reason
                .unwrap_or_else(|| "The extraction plan is incompatible.".into()),
        ));
    }
    let destination = destination_directory(&input_path, request.destination_dir.as_deref())?;
    let stem = input_path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("video");
    let base_name = format!("{stem}-audio");
    let extension = output_extension(&output_format);
    let temporary = temporary_output_path(&destination, &base_name, extension);
    verify_destination_writable(&temporary)?;
    let mut cleanup = TemporaryOutput(Some(temporary.clone()));
    let args = extraction_args(&request, &plan, &input_path, &temporary, extension);
    let phase = if plan.extract_without_reencoding {
        "copying"
    } else {
        "extracting"
    };
    report(AudioExtractionProgress {
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
    let verified = validate_audio_output(&temporary, ffprobe, library_dir)?;
    let final_output = unique_output_path(&destination, &base_name, extension);
    fs::rename(&temporary, &final_output).map_err(|error| {
        AudioExtractionRunError::new("destination_inaccessible", error.to_string())
    })?;
    cleanup.0 = None;
    let final_size = fs::metadata(&final_output)
        .map_err(|error| AudioExtractionRunError::new("output_invalid", error.to_string()))?
        .len();
    report(AudioExtractionProgress {
        path: request.path,
        name: input.name.clone(),
        processed_seconds: input.duration_seconds,
        duration_seconds: input.duration_seconds,
        percent: 100.0,
        phase: phase.into(),
    });
    Ok(ExtractAudioResult {
        source_name: input.name,
        final_format: format_label(&output_format).into(),
        final_codec: verified.codec,
        final_bitrate: verified.bitrate,
        sample_rate: verified.sample_rate,
        channels: verified.channels,
        selected_audio_track: request.selected_audio_track + 1,
        selected_audio_language: track.language,
        extract_without_reencoding: plan.extract_without_reencoding,
        final_size,
        processing_duration_seconds: started_at.elapsed().as_secs_f64(),
        output_name: final_output
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("video-audio")
            .to_owned(),
        output_path: final_output.to_string_lossy().into_owned(),
    })
}

fn output_extension(format: &str) -> &str {
    if format == "ogg" {
        "opus"
    } else {
        format
    }
}

fn format_label(format: &str) -> &str {
    match format {
        "m4a" => "M4A / AAC",
        "ogg" => "OGG / Opus",
        "mp3" => "MP3",
        "wav" => "WAV",
        "flac" => "FLAC",
        _ => "Audio",
    }
}

fn extraction_args(
    request: &ExtractAudioRequest,
    plan: &AudioExtractionPlan,
    input: &Path,
    output: &Path,
    extension: &str,
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
        format!("0:a:{}", request.selected_audio_track).into(),
        "-vn".into(),
    ];
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
    if plan.extract_without_reencoding {
        args.extend(["-c:a".into(), "copy".into()]);
    } else {
        let codec = plan.planned_codec.as_deref().unwrap_or("aac");
        args.extend(["-c:a".into(), codec.into()]);
        if let Some(bitrate) = plan.planned_bitrate {
            args.extend(["-b:a".into(), bitrate.to_string().into()]);
        }
        if let Some(sample_rate) = request.sample_rate {
            args.extend(["-ar".into(), sample_rate.to_string().into()]);
        }
        if let Some(channels) = request.channels {
            args.extend(["-ac".into(), channels.to_string().into()]);
        }
    }
    let muxer = match extension {
        "m4a" => "ipod",
        "opus" => "opus",
        value => value,
    };
    args.extend([
        "-f".into(),
        muxer.into(),
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
    state: &AudioExtractionState,
    args: &[OsString],
    input_path: &str,
    input_name: &str,
    duration: f64,
    phase: &str,
    report: &mut F,
) -> Result<(), AudioExtractionRunError>
where
    F: FnMut(AudioExtractionProgress),
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
        .map_err(|error| AudioExtractionRunError::new("ffmpeg_unavailable", error.to_string()))?;
    let stdout = child.stdout.take().ok_or_else(|| {
        AudioExtractionRunError::new("extraction_failed", "FFmpeg progress is unavailable.")
    })?;
    let mut stderr = child.stderr.take().ok_or_else(|| {
        AudioExtractionRunError::new("extraction_failed", "FFmpeg error output is unavailable.")
    })?;
    {
        let mut active = state.inner.child.lock().map_err(|_| {
            AudioExtractionRunError::new("extraction_failed", "Process lock poisoned.")
        })?;
        if active.is_some() {
            let _ = child.kill();
            return Err(AudioExtractionRunError::new(
                "extraction_busy",
                "Another audio extraction is already running.",
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
                report(AudioExtractionProgress {
                    path: input_path.into(),
                    name: input_name.into(),
                    processed_seconds: processed,
                    duration_seconds: duration,
                    percent: if duration > 0.0 {
                        (processed / duration * 100.0).clamp(0.0, 99.9)
                    } else {
                        0.0
                    },
                    phase: phase.into(),
                });
            }
        }
    }
    let status = state
        .inner
        .child
        .lock()
        .map_err(|_| AudioExtractionRunError::new("extraction_failed", "Process lock poisoned."))?
        .as_mut()
        .ok_or_else(|| AudioExtractionRunError::new("extraction_failed", "Process disappeared."))?
        .wait()
        .map_err(|error| AudioExtractionRunError::new("extraction_failed", error.to_string()))?;
    state
        .inner
        .child
        .lock()
        .map_err(|_| AudioExtractionRunError::new("extraction_failed", "Process lock poisoned."))?
        .take();
    let stderr = stderr_reader.join().unwrap_or_default();
    if state.inner.cancel_requested.swap(false, Ordering::SeqCst) {
        return Err(AudioExtractionRunError::new(
            "extraction_cancelled",
            "Audio extraction was cancelled safely.",
        ));
    }
    if !status.success() {
        return Err(classify_ffmpeg_error(&stderr));
    }
    Ok(())
}

struct VerifiedAudio {
    codec: String,
    bitrate: Option<u64>,
    sample_rate: Option<u64>,
    channels: Option<u64>,
}

fn validate_audio_output(
    path: &Path,
    ffprobe: &Path,
    library_dir: &Path,
) -> Result<VerifiedAudio, AudioExtractionRunError> {
    let size = fs::metadata(path)
        .map_err(|error| AudioExtractionRunError::new("output_invalid", error.to_string()))?
        .len();
    if size == 0 {
        return Err(AudioExtractionRunError::new(
            "output_empty",
            "The output file is empty.",
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
        .map_err(|error| AudioExtractionRunError::new("ffprobe_unavailable", error.to_string()))?;
    if !output.status.success() {
        return Err(AudioExtractionRunError::new(
            "output_invalid",
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    let document: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| AudioExtractionRunError::new("output_invalid", error.to_string()))?;
    let audio = document["streams"]
        .as_array()
        .and_then(|streams| {
            streams
                .iter()
                .find(|stream| stream["codec_type"] == "audio")
        })
        .ok_or_else(|| {
            AudioExtractionRunError::new("output_invalid", "No valid audio track found.")
        })?;
    let parse = |value: &serde_json::Value| {
        value
            .as_str()
            .and_then(|value| value.parse::<u64>().ok())
            .or_else(|| value.as_u64())
    };
    Ok(VerifiedAudio {
        codec: audio["codec_name"]
            .as_str()
            .unwrap_or("unknown")
            .to_ascii_uppercase(),
        bitrate: parse(&audio["bit_rate"]).or_else(|| parse(&document["format"]["bit_rate"])),
        sample_rate: parse(&audio["sample_rate"]),
        channels: parse(&audio["channels"]),
    })
}

fn destination_directory(
    input: &Path,
    requested: Option<&str>,
) -> Result<PathBuf, AudioExtractionRunError> {
    let destination = requested
        .map(PathBuf::from)
        .or_else(|| input.parent().map(Path::to_path_buf))
        .ok_or_else(|| {
            AudioExtractionRunError::new("destination_inaccessible", "Missing destination.")
        })?;
    if !destination.is_dir() {
        return Err(AudioExtractionRunError::new(
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

fn verify_destination_writable(path: &Path) -> Result<(), AudioExtractionRunError> {
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(filesystem_write_error)?;
    fs::remove_file(path).map_err(|error| {
        AudioExtractionRunError::new("destination_inaccessible", error.to_string())
    })
}

fn filesystem_write_error(error: std::io::Error) -> AudioExtractionRunError {
    let message = error.to_string();
    if error.raw_os_error() == Some(112) || message.to_ascii_lowercase().contains("no space left") {
        AudioExtractionRunError::new("disk_full", message)
    } else {
        AudioExtractionRunError::new("destination_inaccessible", message)
    }
}

fn classify_ffmpeg_error(stderr: &str) -> AudioExtractionRunError {
    let lower = stderr.to_ascii_lowercase();
    let code = if lower.contains("no space left") || lower.contains("not enough space") {
        "disk_full"
    } else if lower.contains("unknown encoder") || lower.contains("encoder not found") {
        "codec_unsupported"
    } else if lower.contains("permission denied") || lower.contains("access is denied") {
        "destination_inaccessible"
    } else {
        "extraction_failed"
    };
    AudioExtractionRunError::new(code, stderr.trim())
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

    fn create_source(path: &Path, duration: u64, audio_tracks: usize) {
        let mut command = Command::new(tool("ffmpeg"));
        command.args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=10",
        ]);
        for index in 0..audio_tracks {
            command.args([
                "-f",
                "lavfi",
                "-i",
                &format!(
                    "sine=frequency={}:sample_rate={}",
                    500 + index * 300,
                    if index == 0 { 44_100 } else { 48_000 }
                ),
            ]);
        }
        command.args(["-map", "0:v:0"]);
        for index in 0..audio_tracks {
            command.args(["-map", &format!("{}:a:0", index + 1)]);
        }
        command.args([
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
        ]);
        if audio_tracks > 0 {
            command.args(["-c:a", "aac", "-b:a", "128k", "-shortest"]);
            command.args([
                "-metadata:s:a:0",
                "language=eng",
                "-metadata:s:a:0",
                "title=English",
            ]);
        } else {
            command.arg("-an");
        }
        if audio_tracks > 1 {
            command.args([
                "-metadata:s:a:1",
                "language=fra",
                "-metadata:s:a:1",
                "title=French",
            ]);
        }
        command.args(["-metadata", "title=FileNova extraction test"]);
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

    fn request(path: &Path, destination: &Path, format: &str) -> ExtractAudioRequest {
        ExtractAudioRequest {
            path: path.to_string_lossy().into_owned(),
            destination_dir: Some(destination.to_string_lossy().into_owned()),
            output_format: format.into(),
            quality: "standard".into(),
            custom_bitrate: Some(128),
            sample_rate: None,
            channels: None,
            preserve_metadata: false,
            copy_audio_stream: false,
            selected_audio_track: 0,
        }
    }

    #[test]
    fn arguments_use_only_confirmed_encoders_and_copy_is_exact() {
        let path = Path::new("C:\\source.mp4");
        for (format, codec, extension) in [
            ("mp3", "libmp3lame", "mp3"),
            ("wav", "pcm_s16le", "wav"),
            ("m4a", "aac", "m4a"),
            ("flac", "flac", "flac"),
            ("ogg", "libopus", "opus"),
        ] {
            let request = request(path, Path::new("C:\\"), format);
            let plan = build_audio_extraction_plan(&AudioExtractionPlanRequest {
                input_audio_codec: Some("AAC".into()),
                input_audio_bitrate: Some(128_000),
                input_sample_rate: Some(44_100),
                input_channels: Some(1),
                output_format: format.into(),
                quality: "standard".into(),
                custom_bitrate: Some(128),
                sample_rate: None,
                channels: None,
                copy_audio_stream: false,
            });
            let args = extraction_args(
                &request,
                &plan,
                path,
                Path::new(&format!("C:\\output.{extension}")),
                extension,
            )
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
            assert!(args.contains(&format!("-c:a {codec}")));
            assert!(!args.contains("libx264") && !args.contains("-pass"));
        }
        let mut copy_request = request(path, Path::new("C:\\"), "m4a");
        copy_request.copy_audio_stream = true;
        let copy_plan = build_audio_extraction_plan(&AudioExtractionPlanRequest {
            input_audio_codec: Some("AAC".into()),
            input_audio_bitrate: Some(128_000),
            input_sample_rate: Some(44_100),
            input_channels: Some(1),
            output_format: "m4a".into(),
            quality: "standard".into(),
            custom_bitrate: None,
            sample_rate: None,
            channels: None,
            copy_audio_stream: true,
        });
        let copy = extraction_args(
            &copy_request,
            &copy_plan,
            path,
            Path::new("C:\\out.m4a"),
            "m4a",
        )
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
        assert!(copy.contains("-map 0:a:0 -vn") && copy.contains("-c:a copy"));
    }

    #[test]
    fn really_extracts_every_available_format_and_validates_audio() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("outputs");
        fs::create_dir(&destination).unwrap();
        let source = temp.path().join("video.mp4");
        create_source(&source, 1, 1);
        let original = fs::read(&source).unwrap();
        let state = AudioExtractionState::default();
        for (format, codec, extension) in [
            ("mp3", "MP3", "mp3"),
            ("wav", "PCM_S16LE", "wav"),
            ("m4a", "AAC", "m4a"),
            ("flac", "FLAC", "flac"),
            ("ogg", "OPUS", "opus"),
        ] {
            let result = extract_audio_with_tools(
                request(&source, &destination, format),
                &tool("ffmpeg"),
                &tool("ffprobe"),
                &libraries(),
                &state,
                |_| {},
            )
            .unwrap();
            assert_eq!(result.final_codec, codec);
            assert!(result.final_size > 0 && result.output_name.ends_with(extension));
        }
        assert_eq!(fs::read(&source).unwrap(), original);
        assert!(fs::read_dir(&destination).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp.")));
    }

    #[test]
    fn selects_second_track_resamples_channels_preserves_metadata_and_copies_compatibly() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("outputs");
        fs::create_dir(&destination).unwrap();
        let source = temp.path().join("multitrack.mp4");
        create_source(&source, 1, 2);
        let original = fs::read(&source).unwrap();
        let state = AudioExtractionState::default();

        let mut selected = request(&source, &destination, "m4a");
        selected.selected_audio_track = 1;
        selected.sample_rate = Some(44_100);
        selected.channels = Some(2);
        selected.preserve_metadata = true;
        let result = extract_audio_with_tools(
            selected,
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &state,
            |_| {},
        )
        .unwrap();
        assert_eq!(result.selected_audio_track, 2);
        assert_eq!(result.selected_audio_language.as_deref(), Some("fra"));
        assert_eq!(result.sample_rate, Some(44_100));
        assert_eq!(result.channels, Some(2));
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
        assert!(String::from_utf8_lossy(&metadata.stdout).contains("FileNova extraction test"));

        let mut resampled = request(&source, &destination, "mp3");
        resampled.sample_rate = Some(48_000);
        resampled.channels = Some(1);
        let resampled = extract_audio_with_tools(
            resampled,
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &state,
            |_| {},
        )
        .unwrap();
        assert_eq!(resampled.sample_rate, Some(48_000));
        assert_eq!(resampled.channels, Some(1));

        let mut copy = request(&source, &destination, "m4a");
        copy.copy_audio_stream = true;
        let copied = extract_audio_with_tools(
            copy,
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &state,
            |_| {},
        )
        .unwrap();
        assert!(copied.extract_without_reencoding);
        assert_eq!(copied.final_codec, "AAC");

        let mut incompatible = request(&source, &destination, "mp3");
        incompatible.copy_audio_stream = true;
        let error = extract_audio_with_tools(
            incompatible,
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &state,
            |_| {},
        )
        .unwrap_err();
        assert_eq!(error.code, "incompatible_audio_plan");
        assert_eq!(fs::read(&source).unwrap(), original);
    }

    #[test]
    fn rejects_silent_and_missing_track_continues_batch_and_uses_unique_names() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("outputs");
        fs::create_dir(&destination).unwrap();
        let silent = temp.path().join("silent.mp4");
        let first = temp.path().join("same.mp4");
        let second_dir = temp.path().join("other");
        fs::create_dir(&second_dir).unwrap();
        let second = second_dir.join("same.mp4");
        create_source(&silent, 1, 0);
        create_source(&first, 1, 1);
        create_source(&second, 1, 1);
        let state = AudioExtractionState::default();
        assert_eq!(
            extract_audio_with_tools(
                request(&silent, &destination, "mp3"),
                &tool("ffmpeg"),
                &tool("ffprobe"),
                &libraries(),
                &state,
                |_| {},
            )
            .unwrap_err()
            .code,
            "no_audio_stream"
        );
        let mut missing = request(&first, &destination, "mp3");
        missing.selected_audio_track = 9;
        assert_eq!(
            extract_audio_with_tools(
                missing,
                &tool("ffmpeg"),
                &tool("ffprobe"),
                &libraries(),
                &state,
                |_| {},
            )
            .unwrap_err()
            .code,
            "audio_track_not_found"
        );
        let one = extract_audio_with_tools(
            request(&first, &destination, "mp3"),
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &state,
            |_| {},
        )
        .unwrap();
        let two = extract_audio_with_tools(
            request(&second, &destination, "mp3"),
            &tool("ffmpeg"),
            &tool("ffprobe"),
            &libraries(),
            &state,
            |_| {},
        )
        .unwrap();
        assert_eq!(one.output_name, "same-audio.mp3");
        assert_eq!(two.output_name, "same-audio-2.mp3");
    }

    #[test]
    fn cancellation_stops_only_active_ffmpeg_and_cleans_temporary_output() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("outputs");
        fs::create_dir(&destination).unwrap();
        let source = temp.path().join("long.mp4");
        create_source(&source, 180, 1);
        let original = fs::read(&source).unwrap();
        let state = AudioExtractionState::default();
        let worker_state = state.clone();
        let worker_source = source.clone();
        let worker_destination = destination.clone();
        let worker = thread::spawn(move || {
            extract_audio_with_tools(
                request(&worker_source, &worker_destination, "flac"),
                &tool("ffmpeg"),
                &tool("ffprobe"),
                &libraries(),
                &worker_state,
                |_| {},
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
        assert_eq!(
            worker.join().unwrap().unwrap_err().code,
            "extraction_cancelled"
        );
        assert_eq!(fs::read(&source).unwrap(), original);
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    }
}
