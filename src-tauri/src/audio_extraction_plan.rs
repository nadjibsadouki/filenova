use std::{ffi::OsString, fs, path::Path, process::Command, sync::OnceLock};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::video_tools::{find_sidecar, path_with_runtime_libraries, runtime_library_dir};

const ACCEPTED_VIDEO_EXTENSIONS: [&str; 6] = ["mp4", "mov", "mkv", "avi", "webm", "m4v"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioExtractionCapabilities {
    mp3: bool,
    wav: bool,
    m4a: bool,
    flac: bool,
    ogg_opus: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSourceInfo {
    path: String,
    name: String,
    format: String,
    duration_seconds: f64,
    size: u64,
    audio_codec: Option<String>,
    audio_bitrate: Option<u64>,
    sample_rate: Option<u64>,
    channels: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioExtractionError {
    code: &'static str,
    message: String,
}

impl AudioExtractionError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioExtractionPlanRequest {
    input_audio_codec: Option<String>,
    input_audio_bitrate: Option<u64>,
    input_sample_rate: Option<u64>,
    input_channels: Option<u64>,
    output_format: String,
    quality: String,
    custom_bitrate: Option<u64>,
    sample_rate: Option<u64>,
    channels: Option<u64>,
    copy_audio_stream: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioExtractionPlan {
    valid: bool,
    reason: Option<String>,
    planned_codec: Option<String>,
    planned_bitrate: Option<u64>,
    planned_sample_rate: Option<u64>,
    planned_channels: Option<u64>,
    extract_without_reencoding: bool,
}

fn query_capabilities(
    ffmpeg: &Path,
    library_dir: &Path,
) -> Result<AudioExtractionCapabilities, String> {
    let run = |argument: &str| {
        Command::new(ffmpeg)
            .args(["-hide_banner", argument])
            .env("PATH", path_with_runtime_libraries(library_dir))
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .map_err(|error| error.to_string())
    };
    let encoders = run("-encoders")?;
    let muxers = run("-muxers")?;
    let encoder = |name: &str| {
        encoders
            .lines()
            .any(|line| line.contains(&format!(" {name} ")))
    };
    let muxer = |name: &str| {
        muxers
            .lines()
            .any(|line| line.contains(&format!(" {name} ")))
    };
    Ok(AudioExtractionCapabilities {
        mp3: encoder("libmp3lame") && muxer("mp3"),
        wav: encoder("pcm_s16le") && muxer("wav"),
        m4a: encoder("aac") && muxer("ipod"),
        flac: encoder("flac") && muxer("flac"),
        ogg_opus: encoder("libopus") && (muxer("opus") || muxer("ogg")),
    })
}

fn detect_capabilities(
    ffmpeg: &Path,
    library_dir: &Path,
) -> Result<AudioExtractionCapabilities, String> {
    static CAPABILITIES: OnceLock<Result<AudioExtractionCapabilities, String>> = OnceLock::new();
    CAPABILITIES
        .get_or_init(|| query_capabilities(ffmpeg, library_dir))
        .clone()
}

#[tauri::command]
pub fn get_audio_extraction_capabilities(
    app: AppHandle,
) -> Result<AudioExtractionCapabilities, String> {
    let ffmpeg = find_sidecar(&app, "ffmpeg")?;
    detect_capabilities(&ffmpeg, &runtime_library_dir(&app))
}

#[tauri::command]
pub async fn inspect_audio_source(
    app: AppHandle,
    path: String,
) -> Result<AudioSourceInfo, AudioExtractionError> {
    let ffprobe = find_sidecar(&app, "ffprobe")
        .map_err(|error| AudioExtractionError::new("ffprobe_unavailable", error))?;
    let library_dir = runtime_library_dir(&app);
    tauri::async_runtime::spawn_blocking(move || {
        inspect_audio_source_with_tool(Path::new(&path), &ffprobe, &library_dir)
    })
    .await
    .map_err(|error| AudioExtractionError::new("inspection_failed", error.to_string()))?
}

fn inspect_audio_source_with_tool(
    path: &Path,
    ffprobe: &Path,
    library_dir: &Path,
) -> Result<AudioSourceInfo, AudioExtractionError> {
    if !path.is_absolute() || !path.is_file() {
        return Err(AudioExtractionError::new(
            "video_not_found",
            "The selected video does not exist or is not a local file.",
        ));
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| {
            AudioExtractionError::new("unsupported_video_format", "Missing extension.")
        })?;
    if !ACCEPTED_VIDEO_EXTENSIONS.contains(&extension.as_str()) {
        return Err(AudioExtractionError::new(
            "unsupported_video_format",
            format!("The .{extension} format is not supported."),
        ));
    }
    let metadata = fs::metadata(path)
        .map_err(|error| AudioExtractionError::new("video_read_error", error.to_string()))?;
    if metadata.len() == 0 {
        return Err(AudioExtractionError::new(
            "invalid_video",
            "The selected video is empty.",
        ));
    }
    let output = Command::new(ffprobe)
        .args([
            OsString::from("-v"),
            OsString::from("error"),
            OsString::from("-print_format"),
            OsString::from("json"),
            OsString::from("-show_format"),
            OsString::from("-show_streams"),
            path.as_os_str().to_owned(),
        ])
        .env("PATH", path_with_runtime_libraries(library_dir))
        .output()
        .map_err(|error| AudioExtractionError::new("ffprobe_unavailable", error.to_string()))?;
    if !output.status.success() {
        return Err(AudioExtractionError::new(
            "invalid_video",
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    let document: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| AudioExtractionError::new("invalid_video", error.to_string()))?;
    let streams = document["streams"]
        .as_array()
        .ok_or_else(|| AudioExtractionError::new("invalid_video", "FFprobe found no streams."))?;
    if !streams
        .iter()
        .any(|stream| stream["codec_type"].as_str() == Some("video"))
    {
        return Err(AudioExtractionError::new(
            "unsupported_video_file",
            "The selected file contains no video stream.",
        ));
    }
    let audio = streams
        .iter()
        .find(|stream| stream["codec_type"].as_str() == Some("audio"));
    let parse_u64 = |value: &serde_json::Value| {
        value
            .as_str()
            .and_then(|value| value.parse::<u64>().ok())
            .or_else(|| value.as_u64())
    };
    let duration_seconds = document["format"]["duration"]
        .as_str()
        .or_else(|| audio.and_then(|stream| stream["duration"].as_str()))
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(0.0);
    Ok(AudioSourceInfo {
        path: path.to_string_lossy().into_owned(),
        name: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("video")
            .to_owned(),
        format: if extension == "webm" {
            "WebM".into()
        } else {
            extension.to_ascii_uppercase()
        },
        duration_seconds,
        size: metadata.len(),
        audio_codec: audio
            .and_then(|stream| stream["codec_name"].as_str())
            .map(str::to_ascii_uppercase),
        audio_bitrate: audio.and_then(|stream| parse_u64(&stream["bit_rate"])),
        sample_rate: audio.and_then(|stream| parse_u64(&stream["sample_rate"])),
        channels: audio.and_then(|stream| parse_u64(&stream["channels"])),
    })
}

fn copy_compatible(format: &str, codec: &str) -> bool {
    match format {
        "MP3" => codec == "MP3",
        "WAV" => codec.starts_with("PCM_"),
        "M4A" => codec == "AAC",
        "FLAC" => codec == "FLAC",
        "OGG" => codec == "OPUS",
        _ => false,
    }
}

fn planned_codec(format: &str) -> Option<&'static str> {
    match format {
        "MP3" => Some("libmp3lame"),
        "WAV" => Some("pcm_s16le"),
        "M4A" => Some("aac"),
        "FLAC" => Some("flac"),
        "OGG" => Some("libopus"),
        _ => None,
    }
}

fn quality_bitrate(format: &str, quality: &str) -> Option<u64> {
    let index = match quality {
        "low" => 0,
        "standard" => 1,
        "high" => 2,
        "maximum" => 3,
        _ => 1,
    };
    let values: &[u64] = match format {
        "MP3" => &[96, 128, 192, 320],
        "M4A" => &[96, 128, 192, 256],
        "OGG" => &[64, 96, 160, 256],
        _ => return None,
    };
    Some(values[index] * 1_000)
}

pub fn build_audio_extraction_plan(request: &AudioExtractionPlanRequest) -> AudioExtractionPlan {
    let format = request.output_format.to_ascii_uppercase();
    let Some(input_codec) = request.input_audio_codec.as_deref() else {
        return invalid_plan("no_audio_stream");
    };
    if planned_codec(&format).is_none() {
        return invalid_plan("unsupported_output_format");
    }
    if request
        .sample_rate
        .is_some_and(|value| !(8_000..=192_000).contains(&value))
    {
        return invalid_plan("invalid_sample_rate");
    }
    if request
        .channels
        .is_some_and(|value| !matches!(value, 1 | 2))
    {
        return invalid_plan("invalid_channels");
    }
    if request.copy_audio_stream {
        if !copy_compatible(&format, &input_codec.to_ascii_uppercase()) {
            return invalid_plan("copy_codec_incompatible");
        }
        if request.sample_rate.is_some() || request.channels.is_some() {
            return invalid_plan("copy_transform_incompatible");
        }
        return AudioExtractionPlan {
            valid: true,
            reason: None,
            planned_codec: Some(format!("Copy ({})", input_codec.to_ascii_uppercase())),
            planned_bitrate: request.input_audio_bitrate,
            planned_sample_rate: request.input_sample_rate,
            planned_channels: request.input_channels,
            extract_without_reencoding: true,
        };
    }
    let bitrate = if matches!(format.as_str(), "MP3" | "M4A" | "OGG") {
        request
            .custom_bitrate
            .map(|value| value * 1_000)
            .or_else(|| quality_bitrate(&format, &request.quality))
    } else {
        None
    };
    AudioExtractionPlan {
        valid: true,
        reason: None,
        planned_codec: planned_codec(&format).map(str::to_owned),
        planned_bitrate: bitrate,
        planned_sample_rate: request.sample_rate.or(request.input_sample_rate),
        planned_channels: request.channels.or(request.input_channels),
        extract_without_reencoding: false,
    }
}

fn invalid_plan(reason: &str) -> AudioExtractionPlan {
    AudioExtractionPlan {
        valid: false,
        reason: Some(reason.into()),
        planned_codec: None,
        planned_bitrate: None,
        planned_sample_rate: None,
        planned_channels: None,
        extract_without_reencoding: false,
    }
}

#[tauri::command]
pub fn plan_audio_extraction(request: AudioExtractionPlanRequest) -> AudioExtractionPlan {
    build_audio_extraction_plan(&request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video_tools::TARGET_TRIPLE;
    use std::path::PathBuf;

    fn tool(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!("{name}-{TARGET_TRIPLE}.exe"))
    }

    fn libraries() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/ffmpeg")
    }

    fn create_video(path: &Path, audio: bool) {
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
        if audio {
            command.args(["-f", "lavfi", "-i", "sine=frequency=600:sample_rate=44100"]);
        }
        command.args([
            "-t",
            "1",
            "-c:v",
            "libopenh264",
            "-rc_mode",
            "quality",
            "-qmin",
            "24",
            "-qmax",
            "24",
        ]);
        if audio {
            command.args(["-c:a", "aac", "-b:a", "128k", "-shortest"]);
        } else {
            command.arg("-an");
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

    fn request(format: &str, copy: bool) -> AudioExtractionPlanRequest {
        AudioExtractionPlanRequest {
            input_audio_codec: Some("AAC".into()),
            input_audio_bitrate: Some(128_000),
            input_sample_rate: Some(44_100),
            input_channels: Some(1),
            output_format: format.into(),
            quality: "high".into(),
            custom_bitrate: None,
            sample_rate: None,
            channels: None,
            copy_audio_stream: copy,
        }
    }

    #[test]
    fn detects_every_supported_bundled_audio_encoder_and_muxer() {
        let capabilities = detect_capabilities(&tool("ffmpeg"), &libraries()).unwrap();
        assert!(capabilities.mp3 && capabilities.wav && capabilities.m4a);
        assert!(capabilities.flac && capabilities.ogg_opus);
    }

    #[test]
    fn inspects_real_video_with_audio_and_video_without_audio() {
        let temp = tempfile::tempdir().unwrap();
        let with_audio = temp.path().join("with-audio.mp4");
        let silent = temp.path().join("silent.mp4");
        create_video(&with_audio, true);
        create_video(&silent, false);
        let audio =
            inspect_audio_source_with_tool(&with_audio, &tool("ffprobe"), &libraries()).unwrap();
        assert_eq!(audio.audio_codec.as_deref(), Some("AAC"));
        assert_eq!(audio.sample_rate, Some(44_100));
        assert_eq!(audio.channels, Some(1));
        assert!(audio.audio_bitrate.is_some_and(|value| value > 0));
        let silent =
            inspect_audio_source_with_tool(&silent, &tool("ffprobe"), &libraries()).unwrap();
        assert_eq!(silent.audio_codec, None);
        assert_eq!(silent.sample_rate, None);
    }

    #[test]
    fn validates_copy_compatibility_without_silent_fallback() {
        let m4a = build_audio_extraction_plan(&request("m4a", true));
        assert!(m4a.valid && m4a.extract_without_reencoding);
        assert_eq!(m4a.planned_codec.as_deref(), Some("Copy (AAC)"));
        assert_eq!(
            build_audio_extraction_plan(&request("mp3", true))
                .reason
                .as_deref(),
            Some("copy_codec_incompatible")
        );
        let mut transformed = request("m4a", true);
        transformed.sample_rate = Some(48_000);
        assert_eq!(
            build_audio_extraction_plan(&transformed).reason.as_deref(),
            Some("copy_transform_incompatible")
        );
    }

    #[test]
    fn plans_each_encoder_quality_bitrate_sample_rate_and_channels() {
        for (format, codec) in [
            ("mp3", "libmp3lame"),
            ("wav", "pcm_s16le"),
            ("m4a", "aac"),
            ("flac", "flac"),
            ("ogg", "libopus"),
        ] {
            let mut value = request(format, false);
            value.custom_bitrate = Some(192);
            value.sample_rate = Some(48_000);
            value.channels = Some(2);
            let plan = build_audio_extraction_plan(&value);
            assert!(plan.valid);
            assert_eq!(plan.planned_codec.as_deref(), Some(codec));
            assert_eq!(plan.planned_sample_rate, Some(48_000));
            assert_eq!(plan.planned_channels, Some(2));
            assert_eq!(
                plan.planned_bitrate,
                (!matches!(format, "wav" | "flac")).then_some(192_000)
            );
        }
        let mut silent = request("mp3", false);
        silent.input_audio_codec = None;
        assert_eq!(
            build_audio_extraction_plan(&silent).reason.as_deref(),
            Some("no_audio_stream")
        );
    }
}
