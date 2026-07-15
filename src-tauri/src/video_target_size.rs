const MIN_SAFETY_MARGIN: f64 = 0.04;
const MAX_SAFETY_MARGIN: f64 = 0.06;
const MAX_TARGET_SIZE_BYTES: f64 = 100.0 * 1024.0 * 1024.0 * 1024.0;
pub(crate) const TARGET_SIZE_SAFETY_MARGIN: f64 = 0.06;
/// Conservative empirical floor for OpenH264 bitrate allocation. Synthetic motion samples at
/// 24–30 fps are used in the real encoding tests; this is a selection heuristic, not a quality
/// guarantee. Values below it trigger the next smaller resolution candidate.
pub(crate) const MIN_OPENH264_BITS_PER_PIXEL_PER_FRAME: f64 = 0.08;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TargetResolutionChoice {
    pub width: u64,
    pub height: u64,
    pub bits_per_pixel_per_frame: f64,
    pub reduced: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetSizeUnit {
    Kb,
    Mb,
    Gb,
}

impl TargetSizeUnit {
    fn multiplier(self) -> f64 {
        match self {
            Self::Kb => 1024.0,
            Self::Mb => 1024.0 * 1024.0,
            Self::Gb => 1024.0 * 1024.0 * 1024.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TargetBitrateEstimate {
    pub total_kbps: f64,
    pub video_kbps: f64,
    pub audio_kbps: f64,
    pub overhead_kbps: f64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TargetVideoInput {
    pub duration_seconds: f64,
    pub audio_kbps: f64,
    pub width: u64,
    pub height: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetSizeValidation {
    NonNumeric,
    Zero,
    Negative,
    TooLarge,
    TooSmall,
}

pub(crate) fn target_size_to_bytes(value: f64, unit: TargetSizeUnit) -> f64 {
    value * unit.multiplier()
}

pub(crate) fn calculate_target_bitrates(
    target_bytes: f64,
    duration_seconds: f64,
    audio_kbps: f64,
    safety_margin: f64,
) -> Option<TargetBitrateEstimate> {
    if !target_bytes.is_finite()
        || target_bytes <= 0.0
        || !duration_seconds.is_finite()
        || duration_seconds <= 0.0
        || !audio_kbps.is_finite()
        || audio_kbps < 0.0
        || !(MIN_SAFETY_MARGIN..=MAX_SAFETY_MARGIN).contains(&safety_margin)
    {
        return None;
    }
    let total_kbps = target_bytes * 8.0 / duration_seconds / 1000.0;
    let overhead_kbps = total_kbps * safety_margin;
    Some(TargetBitrateEstimate {
        total_kbps,
        video_kbps: (total_kbps - audio_kbps - overhead_kbps).max(0.0),
        audio_kbps,
        overhead_kbps,
    })
}

pub(crate) fn minimum_usable_video_bitrate_kbps(width: u64, height: u64) -> f64 {
    let pixels = width.saturating_mul(height);
    if pixels >= 1920 * 1080 {
        500.0
    } else if pixels >= 1280 * 720 {
        300.0
    } else if pixels >= 640 * 360 {
        180.0
    } else {
        100.0
    }
}

pub(crate) fn validate_target_size(
    value: f64,
    unit: TargetSizeUnit,
    inputs: &[TargetVideoInput],
) -> Option<TargetSizeValidation> {
    if !value.is_finite() {
        return Some(TargetSizeValidation::NonNumeric);
    }
    if value == 0.0 {
        return Some(TargetSizeValidation::Zero);
    }
    if value < 0.0 {
        return Some(TargetSizeValidation::Negative);
    }
    let target_bytes = target_size_to_bytes(value, unit);
    if !target_bytes.is_finite() || target_bytes > MAX_TARGET_SIZE_BYTES {
        return Some(TargetSizeValidation::TooLarge);
    }
    let too_small = inputs.iter().any(|input| {
        calculate_target_bitrates(
            target_bytes,
            input.duration_seconds,
            input.audio_kbps,
            TARGET_SIZE_SAFETY_MARGIN,
        )
        .is_some_and(|estimate| {
            estimate.video_kbps < minimum_usable_video_bitrate_kbps(input.width, input.height)
        })
    });
    too_small.then_some(TargetSizeValidation::TooSmall)
}

pub(crate) fn target_is_not_smaller(target_bytes: f64, original_bytes: u64) -> bool {
    target_bytes.is_finite() && target_bytes >= original_bytes as f64
}

pub(crate) fn corrected_video_bitrate_kbps(
    first_video_kbps: f64,
    target_bytes: u64,
    actual_bytes: u64,
) -> Option<f64> {
    if !first_video_kbps.is_finite()
        || first_video_kbps <= 0.0
        || target_bytes == 0
        || actual_bytes == 0
    {
        return None;
    }
    Some(first_video_kbps * target_bytes as f64 / actual_bytes as f64 * 0.97)
}

pub(crate) fn bits_per_pixel_per_frame(
    video_bitrate_kbps: f64,
    width: u64,
    height: u64,
    frames_per_second: f64,
) -> Option<f64> {
    if !video_bitrate_kbps.is_finite()
        || video_bitrate_kbps <= 0.0
        || width == 0
        || height == 0
        || !frames_per_second.is_finite()
        || frames_per_second <= 0.0
    {
        return None;
    }
    Some(video_bitrate_kbps * 1000.0 / (width as f64 * height as f64 * frames_per_second))
}

fn fit_even_within(width: u64, height: u64, max_width: u64, max_height: u64) -> (u64, u64) {
    let scale = (max_width as f64 / width as f64)
        .min(max_height as f64 / height as f64)
        .min(1.0);
    let even = |value: f64| ((value.floor() as u64).max(2) / 2) * 2;
    (even(width as f64 * scale), even(height as f64 * scale))
}

pub(crate) fn choose_target_resolution(
    original_width: u64,
    original_height: u64,
    frames_per_second: f64,
    video_bitrate_kbps: f64,
    automatic_reduction: bool,
) -> Option<TargetResolutionChoice> {
    if original_width == 0 || original_height == 0 {
        return None;
    }
    let original = fit_even_within(
        original_width,
        original_height,
        original_width,
        original_height,
    );
    let original_bpppf = bits_per_pixel_per_frame(
        video_bitrate_kbps,
        original.0,
        original.1,
        frames_per_second,
    )?;
    if !automatic_reduction {
        return Some(TargetResolutionChoice {
            width: original.0,
            height: original.1,
            bits_per_pixel_per_frame: original_bpppf,
            reduced: false,
        });
    }

    let portrait = original_height > original_width;
    let standard = [(1920, 1080), (1280, 720), (854, 480), (640, 360)];
    let mut candidates = vec![original];
    for (landscape_width, landscape_height) in standard {
        let bounds = if portrait {
            (landscape_height, landscape_width)
        } else {
            (landscape_width, landscape_height)
        };
        let candidate = fit_even_within(original_width, original_height, bounds.0, bounds.1);
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    let mut selected = *candidates.last().unwrap_or(&original);
    let mut selected_bpppf = bits_per_pixel_per_frame(
        video_bitrate_kbps,
        selected.0,
        selected.1,
        frames_per_second,
    )?;
    for candidate in candidates {
        let bpppf = bits_per_pixel_per_frame(
            video_bitrate_kbps,
            candidate.0,
            candidate.1,
            frames_per_second,
        )?;
        if bpppf >= MIN_OPENH264_BITS_PER_PIXEL_PER_FRAME {
            selected = candidate;
            selected_bpppf = bpppf;
            break;
        }
    }
    Some(TargetResolutionChoice {
        width: selected.0,
        height: selected.1,
        bits_per_pixel_per_frame: selected_bpppf,
        reduced: selected != original,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> TargetVideoInput {
        TargetVideoInput {
            duration_seconds: 60.0,
            audio_kbps: 160.0,
            width: 1920,
            height: 1080,
        }
    }

    #[test]
    fn calculates_total_video_audio_and_six_percent_overhead() {
        let target = target_size_to_bytes(25.0, TargetSizeUnit::Mb);
        let estimate =
            calculate_target_bitrates(target, 60.0, 160.0, TARGET_SIZE_SAFETY_MARGIN).unwrap();
        let expected_total = target * 8.0 / 60.0 / 1000.0;
        assert!((estimate.total_kbps - expected_total).abs() < 0.001);
        assert!((estimate.overhead_kbps - expected_total * 0.06).abs() < 0.001);
        assert!((estimate.video_kbps - (expected_total * 0.94 - 160.0)).abs() < 0.001);
        assert_eq!(estimate.audio_kbps, 160.0);
    }

    #[test]
    fn remove_audio_allocates_all_non_overhead_bitrate_to_video() {
        let target = target_size_to_bytes(10.0, TargetSizeUnit::Mb);
        let estimate = calculate_target_bitrates(target, 60.0, 0.0, 0.06).unwrap();
        assert!((estimate.video_kbps - estimate.total_kbps * 0.94).abs() < 0.001);
        assert_eq!(estimate.audio_kbps, 0.0);
    }

    #[test]
    fn rejects_invalid_margin_duration_and_target_values() {
        assert!(calculate_target_bitrates(1.0, 60.0, 0.0, 0.03).is_none());
        assert!(calculate_target_bitrates(1.0, 0.0, 0.0, 0.05).is_none());
        assert!(calculate_target_bitrates(f64::NAN, 60.0, 0.0, 0.05).is_none());
    }

    #[test]
    fn validates_non_numeric_zero_negative_excessive_and_unusable_targets() {
        assert_eq!(
            validate_target_size(f64::NAN, TargetSizeUnit::Mb, &[]),
            Some(TargetSizeValidation::NonNumeric)
        );
        assert_eq!(
            validate_target_size(0.0, TargetSizeUnit::Mb, &[]),
            Some(TargetSizeValidation::Zero)
        );
        assert_eq!(
            validate_target_size(-1.0, TargetSizeUnit::Mb, &[]),
            Some(TargetSizeValidation::Negative)
        );
        assert_eq!(
            validate_target_size(101.0, TargetSizeUnit::Gb, &[]),
            Some(TargetSizeValidation::TooLarge)
        );
        assert_eq!(
            validate_target_size(1.0, TargetSizeUnit::Mb, &[input()]),
            Some(TargetSizeValidation::TooSmall)
        );
        assert_eq!(
            validate_target_size(25.0, TargetSizeUnit::Mb, &[input()]),
            None
        );
    }

    #[test]
    fn supports_all_units_and_detects_non_reducing_targets() {
        assert_eq!(target_size_to_bytes(1.0, TargetSizeUnit::Kb), 1024.0);
        assert_eq!(
            target_size_to_bytes(1.0, TargetSizeUnit::Gb),
            1024.0_f64.powi(3)
        );
        assert!(target_is_not_smaller(1_000.0, 1_000));
        assert!(!target_is_not_smaller(999.0, 1_000));
    }

    #[test]
    fn recalculates_one_corrective_bitrate_from_actual_size() {
        let corrected = corrected_video_bitrate_kbps(1_000.0, 10_000_000, 12_000_000).unwrap();
        assert!((corrected - 808.333_333).abs() < 0.001);
        assert!(corrected_video_bitrate_kbps(0.0, 1, 1).is_none());
    }

    #[test]
    fn calculates_bits_per_pixel_per_frame() {
        let value = bits_per_pixel_per_frame(2_000.0, 1280, 720, 30.0).unwrap();
        assert!((value - 0.072_337_96).abs() < 0.000_001);
        assert!(bits_per_pixel_per_frame(0.0, 1280, 720, 30.0).is_none());
    }

    #[test]
    fn disabled_resolution_reduction_preserves_original_even_dimensions() {
        let choice = choose_target_resolution(1919, 1079, 30.0, 1_000.0, false).unwrap();
        assert_eq!((choice.width, choice.height), (1918, 1078));
        assert!(!choice.reduced);
    }

    #[test]
    fn selects_4k_to_1080p_and_1080p_to_720p_at_conservative_bitrate() {
        let four_k = choose_target_resolution(3840, 2160, 30.0, 5_000.0, true).unwrap();
        assert_eq!((four_k.width, four_k.height), (1920, 1080));
        let full_hd = choose_target_resolution(1920, 1080, 30.0, 2_500.0, true).unwrap();
        assert_eq!((full_hd.width, full_hd.height), (1280, 720));
    }

    #[test]
    fn adapts_vertical_candidates_without_changing_orientation() {
        let choice = choose_target_resolution(2160, 3840, 30.0, 5_000.0, true).unwrap();
        assert_eq!((choice.width, choice.height), (1080, 1920));
        assert!(choice.height > choice.width);
    }

    #[test]
    fn never_enlarges_small_video_and_preserves_ratio_with_even_dimensions() {
        let choice = choose_target_resolution(500, 280, 30.0, 100.0, true).unwrap();
        assert_eq!((choice.width, choice.height), (500, 280));
        assert!(!choice.reduced);
        let ratio = choose_target_resolution(4000, 3000, 30.0, 2_500.0, true).unwrap();
        assert_eq!(ratio.width % 2, 0);
        assert_eq!(ratio.height % 2, 0);
        assert!((ratio.width as f64 / ratio.height as f64 - 4.0 / 3.0).abs() < 0.01);
        assert!(ratio.width <= 4000 && ratio.height <= 3000);
    }
}
