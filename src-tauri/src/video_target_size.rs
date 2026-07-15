const MIN_SAFETY_MARGIN: f64 = 0.04;
const MAX_SAFETY_MARGIN: f64 = 0.06;
const MAX_TARGET_SIZE_BYTES: f64 = 100.0 * 1024.0 * 1024.0 * 1024.0;
pub(crate) const TARGET_SIZE_SAFETY_MARGIN: f64 = 0.06;

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
}
