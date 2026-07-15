const MIN_SELECTION_SECONDS: f64 = 0.1;

use std::path::Path;
use tauri::{AppHandle, Manager};

#[tauri::command]
pub fn allow_trim_video_preview(app: AppHandle, path: String) -> Result<(), String> {
    let path = Path::new(&path);
    if !path.is_absolute() || !path.is_file() {
        return Err("The selected video does not exist or is not a local file.".into());
    }
    app.asset_protocol_scope()
        .allow_file(path)
        .map_err(|error| error.to_string())
}

pub(crate) fn parse_timecode(value: &str) -> Result<f64, &'static str> {
    let value = value.trim();
    let (negative, value) = value
        .strip_prefix('-')
        .map_or((false, value), |value| (true, value));
    let parts = value.split(':').collect::<Vec<_>>();
    if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
        return Err("invalid_time_format");
    }
    let seconds_parts = parts[2].split('.').collect::<Vec<_>>();
    if parts[0].len() != 2
        || parts[1].len() != 2
        || seconds_parts[0].len() != 2
        || seconds_parts.len() > 2
        || seconds_parts
            .get(1)
            .is_some_and(|value| value.is_empty() || value.len() > 3)
    {
        return Err("invalid_time_format");
    }
    let hours = parts[0].parse::<u64>().map_err(|_| "invalid_time_format")?;
    let minutes = parts[1].parse::<u64>().map_err(|_| "invalid_time_format")?;
    let seconds = parts[2].parse::<f64>().map_err(|_| "invalid_time_format")?;
    if minutes >= 60 || !seconds.is_finite() || !(0.0..60.0).contains(&seconds) {
        return Err("invalid_time_format");
    }
    let result = hours as f64 * 3_600.0 + minutes as f64 * 60.0 + seconds;
    Ok(if negative { -result } else { result })
}

pub(crate) fn validate_trim_selection(
    start: f64,
    end: f64,
    duration: f64,
) -> Result<(), &'static str> {
    if !start.is_finite() || !end.is_finite() || !duration.is_finite() {
        return Err("invalid_time_format");
    }
    if start < 0.0 {
        return Err("start_negative");
    }
    if end > duration + 0.000_5 {
        return Err("end_after_duration");
    }
    if start >= end {
        return Err("start_not_before_end");
    }
    if end - start < MIN_SELECTION_SECONDS - 0.000_5 {
        return Err("selection_too_short");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video_tools::{
        inspect_video_with_tools, path_with_runtime_libraries, TARGET_TRIPLE,
    };
    use std::{path::PathBuf, process::Command};

    fn tool(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!("{name}-{TARGET_TRIPLE}.exe"))
    }

    fn libraries() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/ffmpeg")
    }

    fn create_video(path: &Path, duration: u64) {
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
                "sine=frequency=500:sample_rate=44100",
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
                "-c:a",
                "aac",
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

    #[test]
    fn parses_hh_mm_ss_and_milliseconds_exactly() {
        assert_eq!(parse_timecode("00:00:05").unwrap(), 5.0);
        assert_eq!(parse_timecode("00:01:05.250").unwrap(), 65.25);
        assert_eq!(parse_timecode("02:03:04.005").unwrap(), 7_384.005);
        for invalid in ["5", "00:70:00", "00:00:60", "-1:00:00", "aa:00:00"] {
            assert_eq!(parse_timecode(invalid), Err("invalid_time_format"));
        }
        assert_eq!(parse_timecode("-00:00:01.250").unwrap(), -1.25);
        assert_eq!(
            validate_trim_selection(parse_timecode("-00:00:01").unwrap(), 1.0, 2.0),
            Err("start_negative")
        );
    }

    #[test]
    fn validates_short_and_long_video_boundaries() {
        assert!(validate_trim_selection(0.0, 0.1, 2.0).is_ok());
        assert!(validate_trim_selection(60.125, 7_322.875, 7_323.0).is_ok());
        assert_eq!(
            validate_trim_selection(-0.001, 1.0, 2.0),
            Err("start_negative")
        );
        assert_eq!(
            validate_trim_selection(1.0, 1.0, 2.0),
            Err("start_not_before_end")
        );
        assert_eq!(
            validate_trim_selection(1.0, 1.099, 2.0),
            Err("selection_too_short")
        );
        assert_eq!(
            validate_trim_selection(0.0, 2.001, 2.0),
            Err("end_after_duration")
        );
    }

    #[test]
    fn preview_boundaries_remain_inside_the_selected_segment() {
        let start = parse_timecode("00:00:01.250").unwrap();
        let end = parse_timecode("00:00:03.750").unwrap();
        validate_trim_selection(start, end, 5.0).unwrap();
        let playback_positions = [start, 2.0, 3.749, end];
        assert!(playback_positions[..3]
            .iter()
            .all(|position| *position >= start && *position < end));
        assert!(playback_positions[3] >= end);
    }

    #[test]
    fn ffprobe_analyzes_real_short_and_long_trim_sources() {
        let temp = tempfile::tempdir().unwrap();
        let short = temp.path().join("short.mp4");
        let long = temp.path().join("long.mp4");
        create_video(&short, 1);
        create_video(&long, 12);
        let short_info =
            inspect_video_with_tools(&short, &tool("ffprobe"), &tool("ffmpeg"), &libraries())
                .unwrap();
        let long_info =
            inspect_video_with_tools(&long, &tool("ffprobe"), &tool("ffmpeg"), &libraries())
                .unwrap();
        assert!((short_info.duration_seconds - 1.0).abs() < 0.1);
        assert!((long_info.duration_seconds - 12.0).abs() < 0.1);
        assert!(validate_trim_selection(0.0, 0.5, short_info.duration_seconds).is_ok());
        assert!(validate_trim_selection(1.25, 10.75, long_info.duration_seconds).is_ok());
    }
}
