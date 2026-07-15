use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoConversionPlanRequest {
    pub(crate) input_video_codec: String,
    pub(crate) input_audio_codec: Option<String>,
    pub(crate) output_format: String,
    pub(crate) video_codec: String,
    pub(crate) audio_codec: String,
    pub(crate) preserve_resolution: bool,
    pub(crate) preserve_frame_rate: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VideoConversionPlan {
    pub(crate) valid: bool,
    pub(crate) reason: Option<String>,
    pub(crate) planned_video_codec: String,
    pub(crate) planned_audio_codec: Option<String>,
    pub(crate) remux_without_reencoding: bool,
}

pub(crate) fn normalized(value: &str) -> String {
    value.trim().to_ascii_uppercase().replace('-', "")
}

pub(crate) fn container_supports_video(container: &str, codec: &str) -> bool {
    match container {
        "MP4" => matches!(codec, "H264" | "HEVC" | "AV1" | "MPEG4"),
        "MOV" => matches!(codec, "H264" | "HEVC" | "MPEG4" | "PRORES"),
        "WEBM" => matches!(codec, "VP8" | "VP9" | "AV1"),
        "MKV" => codec != "UNKNOWN" && !codec.is_empty(),
        _ => false,
    }
}

pub(crate) fn container_supports_audio(container: &str, codec: &str) -> bool {
    match container {
        "MP4" | "MOV" => matches!(codec, "AAC" | "MP3" | "ALAC" | "OPUS"),
        "WEBM" => matches!(codec, "OPUS" | "VORBIS"),
        "MKV" => codec != "UNKNOWN" && !codec.is_empty(),
        _ => false,
    }
}

pub fn build_video_conversion_plan(request: &VideoConversionPlanRequest) -> VideoConversionPlan {
    let container = normalized(&request.output_format);
    let input_video = normalized(&request.input_video_codec);
    let input_audio = request.input_audio_codec.as_deref().map(normalized);
    let requested_video = normalized(&request.video_codec);
    let requested_audio = normalized(&request.audio_codec);

    let can_copy_video = container_supports_video(&container, &input_video)
        && request.preserve_resolution
        && request.preserve_frame_rate;
    let planned_video = match requested_video.as_str() {
        "AUTO" if matches!(container.as_str(), "MP4" | "MOV") && can_copy_video => "Copy",
        "AUTO" if container == "WEBM" => "SVT-AV1",
        "AUTO" => "OpenH264",
        "OPENH264" => "OpenH264",
        "SVTAV1" => "SVT-AV1",
        "COPY" => "Copy",
        _ => {
            return invalid_plan("unsupported_video_codec", "", None);
        }
    };
    let can_copy_audio = input_audio
        .as_deref()
        .is_some_and(|codec| container_supports_audio(&container, codec));
    let planned_audio = match requested_audio.as_str() {
        "NOAUDIO" => None,
        "AUTO" if input_audio.is_none() => None,
        "AUTO" if matches!(container.as_str(), "MP4" | "MOV") && can_copy_audio => Some("Copy"),
        "AUTO" if container == "WEBM" => Some("Opus"),
        "AUTO" => Some("AAC"),
        "AAC" => Some("AAC"),
        "OPUS" => Some("Opus"),
        "COPY" if input_audio.is_some() => Some("Copy"),
        "COPY" => return invalid_plan("no_audio_stream_to_copy", planned_video, None),
        _ => return invalid_plan("unsupported_audio_codec", planned_video, None),
    };

    if planned_video == "OpenH264" && !matches!(container.as_str(), "MP4" | "MOV" | "MKV") {
        return invalid_plan(
            "openh264_container_incompatible",
            planned_video,
            planned_audio,
        );
    }
    if planned_video == "SVT-AV1" && !matches!(container.as_str(), "MP4" | "WEBM" | "MKV") {
        return invalid_plan("av1_container_incompatible", planned_video, planned_audio);
    }
    if planned_video == "Copy"
        && (!container_supports_video(&container, &input_video)
            || !request.preserve_resolution
            || !request.preserve_frame_rate)
    {
        let reason = if !request.preserve_resolution || !request.preserve_frame_rate {
            "copy_video_transform_incompatible"
        } else {
            "copy_video_container_incompatible"
        };
        return invalid_plan(reason, planned_video, planned_audio);
    }

    if let Some(audio) = planned_audio {
        if audio == "AAC" && !matches!(container.as_str(), "MP4" | "MOV" | "MKV") {
            return invalid_plan("aac_container_incompatible", planned_video, Some(audio));
        }
        if audio == "Opus" && !matches!(container.as_str(), "MP4" | "MOV" | "WEBM" | "MKV") {
            return invalid_plan("opus_container_incompatible", planned_video, Some(audio));
        }
        if audio == "Copy"
            && !container_supports_audio(&container, input_audio.as_deref().unwrap_or(""))
        {
            return invalid_plan(
                "copy_audio_container_incompatible",
                planned_video,
                Some(audio),
            );
        }
    }

    let remux = planned_video == "Copy"
        && (planned_audio == Some("Copy") || planned_audio.is_none())
        && request.preserve_resolution
        && request.preserve_frame_rate;
    VideoConversionPlan {
        valid: true,
        reason: None,
        planned_video_codec: if planned_video == "Copy" {
            format!("Copy ({input_video})")
        } else {
            planned_video.to_owned()
        },
        planned_audio_codec: planned_audio.map(|audio| {
            if audio == "Copy" {
                format!("Copy ({})", input_audio.as_deref().unwrap_or(""))
            } else {
                audio.to_owned()
            }
        }),
        remux_without_reencoding: remux,
    }
}

fn invalid_plan(reason: &str, video: &str, audio: Option<&str>) -> VideoConversionPlan {
    VideoConversionPlan {
        valid: false,
        reason: Some(reason.to_owned()),
        planned_video_codec: video.to_owned(),
        planned_audio_codec: audio.map(str::to_owned),
        remux_without_reencoding: false,
    }
}

#[tauri::command]
pub fn plan_video_conversion(request: VideoConversionPlanRequest) -> VideoConversionPlan {
    build_video_conversion_plan(&request)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(output: &str, video: &str, audio: &str) -> VideoConversionPlanRequest {
        VideoConversionPlanRequest {
            input_video_codec: "H264".into(),
            input_audio_codec: Some("AAC".into()),
            output_format: output.into(),
            video_codec: video.into(),
            audio_codec: audio.into(),
            preserve_resolution: true,
            preserve_frame_rate: true,
        }
    }

    #[test]
    fn resolves_auto_only_to_encoders_present_in_the_bundled_build() {
        let mp4 = build_video_conversion_plan(&request("mp4", "auto", "auto"));
        assert!(mp4.valid);
        assert_eq!(mp4.planned_video_codec, "Copy (H264)");
        assert_eq!(mp4.planned_audio_codec.as_deref(), Some("Copy (AAC)"));
        assert!(mp4.remux_without_reencoding);

        let mut incompatible = request("mp4", "auto", "auto");
        incompatible.input_video_codec = "VP9".into();
        incompatible.input_audio_codec = Some("VORBIS".into());
        let encoded = build_video_conversion_plan(&incompatible);
        assert_eq!(encoded.planned_video_codec, "OpenH264");
        assert_eq!(encoded.planned_audio_codec.as_deref(), Some("AAC"));
        assert!(!encoded.remux_without_reencoding);

        let webm = build_video_conversion_plan(&request("webm", "auto", "auto"));
        assert!(webm.valid);
        assert_eq!(webm.planned_video_codec, "SVT-AV1");
        assert_eq!(webm.planned_audio_codec.as_deref(), Some("Opus"));
    }

    #[test]
    fn rejects_invalid_codec_and_container_combinations() {
        assert_eq!(
            build_video_conversion_plan(&request("webm", "openh264", "opus"))
                .reason
                .as_deref(),
            Some("openh264_container_incompatible")
        );
        assert_eq!(
            build_video_conversion_plan(&request("webm", "svtAv1", "aac"))
                .reason
                .as_deref(),
            Some("aac_container_incompatible")
        );
        assert_eq!(
            build_video_conversion_plan(&request("mov", "svtAv1", "aac"))
                .reason
                .as_deref(),
            Some("av1_container_incompatible")
        );
    }

    #[test]
    fn remux_requires_copyable_streams_and_no_video_transform() {
        let copy = build_video_conversion_plan(&request("mp4", "copy", "copy"));
        assert!(copy.valid && copy.remux_without_reencoding);

        let mut webm = request("webm", "copy", "copy");
        assert_eq!(
            build_video_conversion_plan(&webm).reason.as_deref(),
            Some("copy_video_container_incompatible")
        );
        webm.input_video_codec = "VP9".into();
        webm.input_audio_codec = Some("OPUS".into());
        assert!(build_video_conversion_plan(&webm).remux_without_reencoding);

        let mut resized = request("mp4", "copy", "copy");
        resized.preserve_resolution = false;
        assert_eq!(
            build_video_conversion_plan(&resized).reason.as_deref(),
            Some("copy_video_transform_incompatible")
        );
    }

    #[test]
    fn handles_missing_audio_and_mkv_copy_safely() {
        let mut silent = request("mkv", "copy", "auto");
        silent.input_video_codec = "VP9".into();
        silent.input_audio_codec = None;
        let plan = build_video_conversion_plan(&silent);
        assert!(plan.valid && plan.planned_audio_codec.is_none());

        silent.audio_codec = "copy".into();
        assert_eq!(
            build_video_conversion_plan(&silent).reason.as_deref(),
            Some("no_audio_stream_to_copy")
        );
    }
}
