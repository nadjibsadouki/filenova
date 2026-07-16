use std::path::PathBuf;

use tauri::{AppHandle, Manager};

#[tauri::command]
pub fn allow_video_gif_preview(app: AppHandle, path: String) -> Result<(), String> {
    let path = PathBuf::from(path)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| "Missing video extension.".to_owned())?;
    if !["mp4", "mov", "mkv", "avi", "webm", "m4v"].contains(&extension.as_str()) {
        return Err("Unsupported Video to GIF preview format.".into());
    }
    app.asset_protocol_scope()
        .allow_file(&path)
        .map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GifDimensions {
    pub(crate) width: u64,
    pub(crate) height: u64,
}

pub(crate) fn calculate_dimensions(
    source_width: u64,
    source_height: u64,
    requested_width: f64,
    keep_aspect_ratio: bool,
) -> Option<GifDimensions> {
    if source_width < 2
        || source_height < 2
        || !requested_width.is_finite()
        || requested_width < 2.0
    {
        return None;
    }
    let even_floor = |value: f64| ((value.floor() as u64).max(2) / 2) * 2;
    let even_nearest = |value: f64| (((value / 2.0).round() as u64).max(1)) * 2;
    let width = even_floor(requested_width.min(source_width as f64));
    let height = if keep_aspect_ratio {
        even_nearest(source_height as f64 * width as f64 / source_width as f64)
    } else {
        even_floor(source_height as f64)
    };
    Some(GifDimensions { width, height })
}

pub(crate) fn resolve_requested_width(
    source_width: u64,
    dimension: &str,
    custom_width: Option<f64>,
) -> Option<f64> {
    match dimension {
        "original" => (source_width >= 2).then_some(source_width as f64),
        "1080" => Some(1080.0),
        "720" => Some(720.0),
        "480" => Some(480.0),
        "360" => Some(360.0),
        "custom" => custom_width.filter(|width| width.is_finite() && *width >= 2.0),
        _ => None,
    }
}

pub(crate) fn estimate_frames(duration_seconds: f64, frames_per_second: f64) -> Option<u64> {
    if !duration_seconds.is_finite()
        || !frames_per_second.is_finite()
        || duration_seconds <= 0.0
        || frames_per_second <= 0.0
    {
        return None;
    }
    Some((duration_seconds * frames_per_second).ceil() as u64)
}

pub(crate) fn validate_frame_rate(frames_per_second: f64) -> Result<(), &'static str> {
    if frames_per_second.is_finite() && (1.0..=120.0).contains(&frames_per_second) {
        Ok(())
    } else {
        Err("invalid_frame_rate")
    }
}

pub(crate) fn validate_selection(start: f64, end: f64, duration: f64) -> Result<(), &'static str> {
    if !start.is_finite() || !end.is_finite() || !duration.is_finite() {
        return Err("invalid_time");
    }
    if start < 0.0 {
        return Err("negative_start");
    }
    if start >= end {
        return Err("start_after_end");
    }
    if end > duration + 0.000_5 {
        return Err("end_after_duration");
    }
    if end - start < 0.099_5 {
        return Err("selection_too_short");
    }
    if end - start > 30.000_5 {
        return Err("selection_too_long");
    }
    Ok(())
}

pub(crate) fn parse_time(value: &str) -> Option<f64> {
    let (main, milliseconds) = value.trim().split_once('.').unwrap_or((value.trim(), "0"));
    let parts = main.split(':').collect::<Vec<_>>();
    if parts.len() != 3 || milliseconds.is_empty() || milliseconds.len() > 3 {
        return None;
    }
    let hours = parts[0].parse::<u64>().ok()?;
    let minutes = parts[1].parse::<u64>().ok()?;
    let seconds = parts[2].parse::<u64>().ok()?;
    if parts[0].len() != 2
        || parts[1].len() != 2
        || parts[2].len() != 2
        || minutes > 59
        || seconds > 59
    {
        return None;
    }
    let fraction =
        milliseconds.parse::<u64>().ok()? as f64 / 10_u64.pow(milliseconds.len() as u32) as f64;
    Some((hours * 3600 + minutes * 60 + seconds) as f64 + fraction)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    use crate::video_tools::{
        inspect_video_with_tools, path_with_runtime_libraries, TARGET_TRIPLE,
    };

    fn tool(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!("{name}-{TARGET_TRIPLE}.exe"))
    }

    fn libraries() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/ffmpeg")
    }

    fn create_video(path: &std::path::Path, width: u64, height: u64) {
        let output = Command::new(tool("ffmpeg"))
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                &format!("testsrc2=size={width}x{height}:rate=15"),
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

    #[test]
    fn calculates_horizontal_and_vertical_dimensions_without_upscaling() {
        assert_eq!(
            calculate_dimensions(1920, 1080, 720.0, true),
            Some(GifDimensions {
                width: 720,
                height: 406
            })
        );
        assert_eq!(
            calculate_dimensions(1080, 1920, 480.0, true),
            Some(GifDimensions {
                width: 480,
                height: 854
            })
        );
        assert_eq!(
            calculate_dimensions(320, 180, 1080.0, true),
            Some(GifDimensions {
                width: 320,
                height: 180
            })
        );
    }

    #[test]
    fn resolves_every_dimension_mode_and_rejects_invalid_widths() {
        assert_eq!(resolve_requested_width(641, "original", None), Some(641.0));
        assert_eq!(resolve_requested_width(641, "1080", None), Some(1080.0));
        assert_eq!(resolve_requested_width(641, "720", None), Some(720.0));
        assert_eq!(resolve_requested_width(641, "480", None), Some(480.0));
        assert_eq!(resolve_requested_width(641, "360", None), Some(360.0));
        assert_eq!(
            resolve_requested_width(641, "custom", Some(422.0)),
            Some(422.0)
        );
        assert_eq!(resolve_requested_width(641, "custom", Some(0.0)), None);
        assert_eq!(resolve_requested_width(641, "custom", Some(-20.0)), None);
        assert_eq!(resolve_requested_width(641, "custom", Some(f64::NAN)), None);
        assert_eq!(resolve_requested_width(641, "unknown", None), None);
        assert_eq!(calculate_dimensions(640, 360, 0.0, true), None);
        assert_eq!(calculate_dimensions(640, 360, -10.0, true), None);
    }

    #[test]
    fn parses_whole_seconds_and_milliseconds() {
        assert_eq!(parse_time("00:00:07"), Some(7.0));
        assert_eq!(parse_time("01:02:03.125"), Some(3723.125));
        assert_eq!(parse_time("00:60:00"), None);
    }

    #[test]
    fn validates_timeline_limits_and_preview_segment() {
        assert_eq!(validate_selection(1.125, 15.875, 60.0), Ok(()));
        assert_eq!(validate_selection(0.0, 30.0, 60.0), Ok(()));
        assert_eq!(
            validate_selection(0.0, 30.001, 60.0),
            Err("selection_too_long")
        );
        assert_eq!(
            validate_selection(4.0, 4.05, 60.0),
            Err("selection_too_short")
        );
        let preview_times = [1.125, 4.0, 15.874];
        assert!(preview_times
            .iter()
            .all(|time| *time >= 1.125 && *time <= 15.875));
    }

    #[test]
    fn estimates_approximate_frame_counts() {
        assert_eq!(estimate_frames(10.0, 15.0), Some(150));
        assert_eq!(estimate_frames(2.001, 24.0), Some(49));
        assert_eq!(estimate_frames(10.0, 0.0), None);
    }

    #[test]
    fn accepts_all_frame_rate_presets_and_custom_values() {
        for frames_per_second in [5.0, 10.0, 15.0, 20.0, 24.0, 30.0, 17.5, 120.0] {
            assert_eq!(validate_frame_rate(frames_per_second), Ok(()));
        }
        for frames_per_second in [0.0, -1.0, 120.1, f64::NAN, f64::INFINITY] {
            assert_eq!(
                validate_frame_rate(frames_per_second),
                Err("invalid_frame_rate")
            );
        }
    }

    #[test]
    fn ffprobe_analyzes_real_horizontal_and_vertical_sources_for_gif_planning() {
        let temp = tempfile::tempdir().unwrap();
        let horizontal_path = temp.path().join("horizontal.mp4");
        let vertical_path = temp.path().join("vertical.mp4");
        create_video(&horizontal_path, 640, 360);
        create_video(&vertical_path, 360, 640);
        let horizontal = inspect_video_with_tools(
            &horizontal_path,
            &tool("ffprobe"),
            &tool("ffmpeg"),
            &libraries(),
        )
        .unwrap();
        let vertical = inspect_video_with_tools(
            &vertical_path,
            &tool("ffprobe"),
            &tool("ffmpeg"),
            &libraries(),
        )
        .unwrap();
        assert_eq!((horizontal.width, horizontal.height), (640, 360));
        assert_eq!((vertical.width, vertical.height), (360, 640));
        assert!((horizontal.frames_per_second - 15.0).abs() < 0.01);
        assert_eq!(
            calculate_dimensions(vertical.width, vertical.height, 480.0, true),
            Some(GifDimensions {
                width: 360,
                height: 640
            })
        );
    }
}
