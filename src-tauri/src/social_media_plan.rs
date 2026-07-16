#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdaptationMethod {
    Fit,
    Fill,
    Stretch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SocialPreset {
    pub id: &'static str,
    pub width: u32,
    pub height: u32,
}

pub(crate) const PRESETS: [SocialPreset; 15] = [
    SocialPreset {
        id: "tiktok-vertical",
        width: 1080,
        height: 1920,
    },
    SocialPreset {
        id: "instagram-reel",
        width: 1080,
        height: 1920,
    },
    SocialPreset {
        id: "instagram-square",
        width: 1080,
        height: 1080,
    },
    SocialPreset {
        id: "instagram-portrait",
        width: 1080,
        height: 1350,
    },
    SocialPreset {
        id: "instagram-landscape",
        width: 1080,
        height: 566,
    },
    SocialPreset {
        id: "youtube-short",
        width: 1080,
        height: 1920,
    },
    SocialPreset {
        id: "youtube-standard",
        width: 1920,
        height: 1080,
    },
    SocialPreset {
        id: "facebook-reel",
        width: 1080,
        height: 1920,
    },
    SocialPreset {
        id: "facebook-feed",
        width: 1280,
        height: 720,
    },
    SocialPreset {
        id: "facebook-square",
        width: 1080,
        height: 1080,
    },
    SocialPreset {
        id: "linkedin-landscape",
        width: 1920,
        height: 1080,
    },
    SocialPreset {
        id: "linkedin-square",
        width: 1080,
        height: 1080,
    },
    SocialPreset {
        id: "linkedin-vertical",
        width: 1080,
        height: 1350,
    },
    SocialPreset {
        id: "x-landscape",
        width: 1280,
        height: 720,
    },
    SocialPreset {
        id: "x-square",
        width: 1080,
        height: 1080,
    },
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AdaptationPlan {
    pub content_width: u32,
    pub content_height: u32,
    pub crop_x: u32,
    pub crop_y: u32,
    pub distorted: bool,
    pub upscale_warning: bool,
}

pub(crate) fn plan_adaptation(
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    method: AdaptationMethod,
) -> Option<AdaptationPlan> {
    if [source_width, source_height, target_width, target_height].contains(&0) {
        return None;
    }
    let width_scale = target_width as f64 / source_width as f64;
    let height_scale = target_height as f64 / source_height as f64;
    let upscale_warning = width_scale > 1.0 || height_scale > 1.0;
    match method {
        AdaptationMethod::Stretch => Some(AdaptationPlan {
            content_width: target_width,
            content_height: target_height,
            crop_x: 0,
            crop_y: 0,
            distorted: (source_width as f64 / source_height as f64
                - target_width as f64 / target_height as f64)
                .abs()
                > 0.0001,
            upscale_warning,
        }),
        AdaptationMethod::Fit | AdaptationMethod::Fill => {
            let scale = if method == AdaptationMethod::Fit {
                width_scale.min(height_scale)
            } else {
                width_scale.max(height_scale)
            };
            let content_width = (source_width as f64 * scale).round().max(1.0) as u32;
            let content_height = (source_height as f64 * scale).round().max(1.0) as u32;
            Some(AdaptationPlan {
                content_width,
                content_height,
                crop_x: content_width.saturating_sub(target_width),
                crop_y: content_height.saturating_sub(target_height),
                distorted: false,
                upscale_warning,
            })
        }
    }
}

pub(crate) fn valid_custom_configuration(width: i64, height: i64, frame_rate: f64) -> bool {
    width > 0 && height > 0 && frame_rate.is_finite() && frame_rate > 0.0 && frame_rate <= 120.0
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SafeZone {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

pub(crate) fn safe_zone(name: &str) -> Option<SafeZone> {
    match name {
        "tiktok" => Some(SafeZone {
            left: 0.06,
            top: 0.07,
            right: 0.94,
            bottom: 0.88,
        }),
        "instagram" => Some(SafeZone {
            left: 0.07,
            top: 0.08,
            right: 0.93,
            bottom: 0.85,
        }),
        "youtube" => Some(SafeZone {
            left: 0.08,
            top: 0.06,
            right: 0.92,
            bottom: 0.82,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        process::Command,
    };

    use super::*;
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

    fn create_video(path: &Path, width: u32, height: u32) {
        let output = Command::new(tool("ffmpeg"))
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!("testsrc=size={width}x{height}:rate=15"))
            .args(["-t", "0.4", "-c:v", "mpeg4", "-an"])
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
    fn exposes_every_required_platform_preset() {
        let expected = [
            ("tiktok-vertical", 1080, 1920),
            ("instagram-reel", 1080, 1920),
            ("instagram-square", 1080, 1080),
            ("instagram-portrait", 1080, 1350),
            ("instagram-landscape", 1080, 566),
            ("youtube-short", 1080, 1920),
            ("youtube-standard", 1920, 1080),
            ("facebook-reel", 1080, 1920),
            ("facebook-feed", 1280, 720),
            ("facebook-square", 1080, 1080),
            ("linkedin-landscape", 1920, 1080),
            ("linkedin-square", 1080, 1080),
            ("linkedin-vertical", 1080, 1350),
            ("x-landscape", 1280, 720),
            ("x-square", 1080, 1080),
        ];
        assert_eq!(PRESETS.len(), expected.len());
        for (id, width, height) in expected {
            assert!(PRESETS.contains(&SocialPreset { id, width, height }));
        }
    }

    #[test]
    fn fit_fill_and_stretch_have_the_required_geometry() {
        let fit = plan_adaptation(1920, 1080, 1080, 1920, AdaptationMethod::Fit).unwrap();
        assert_eq!((fit.content_width, fit.content_height), (1080, 608));
        assert_eq!((fit.crop_x, fit.crop_y), (0, 0));
        assert!(!fit.distorted);

        let fill = plan_adaptation(1920, 1080, 1080, 1920, AdaptationMethod::Fill).unwrap();
        assert_eq!((fill.content_width, fill.content_height), (3413, 1920));
        assert_eq!((fill.crop_x, fill.crop_y), (2333, 0));
        assert!(!fill.distorted);

        let stretch = plan_adaptation(1920, 1080, 1080, 1920, AdaptationMethod::Stretch).unwrap();
        assert_eq!(
            (stretch.content_width, stretch.content_height),
            (1080, 1920)
        );
        assert!(stretch.distorted);
        assert!(plan_adaptation(0, 1080, 1080, 1920, AdaptationMethod::Fit).is_none());
    }

    #[test]
    fn validates_custom_values_and_preview_safe_zones() {
        assert!(valid_custom_configuration(1080, 1350, 29.97));
        assert!(!valid_custom_configuration(0, 1350, 30.0));
        assert!(!valid_custom_configuration(1080, -1, 30.0));
        assert!(!valid_custom_configuration(1080, 1350, 0.0));
        for name in ["tiktok", "instagram", "youtube"] {
            let zone = safe_zone(name).unwrap();
            assert!(zone.left >= 0.0 && zone.top >= 0.0 && zone.right <= 1.0 && zone.bottom <= 1.0);
            assert!(zone.left < zone.right && zone.top < zone.bottom);
        }
        assert!(safe_zone("unknown").is_none());
    }

    #[test]
    fn ffprobe_analyzes_real_horizontal_and_vertical_social_sources() {
        let temp = tempfile::tempdir().unwrap();
        for (name, width, height) in [("horizontal.mp4", 640, 360), ("vertical.mp4", 360, 640)] {
            let path = temp.path().join(name);
            create_video(&path, width, height);
            let info =
                inspect_video_with_tools(&path, &tool("ffprobe"), &tool("ffmpeg"), &libraries())
                    .unwrap();
            assert_eq!((info.width, info.height), (width as u64, height as u64));
            assert!((info.frames_per_second - 15.0).abs() < 0.01);
            let plan = plan_adaptation(width, height, 1080, 1920, AdaptationMethod::Fill).unwrap();
            assert!(!plan.distorted);
            assert!(plan.content_width >= 1080 && plan.content_height >= 1920);
        }
    }
}
