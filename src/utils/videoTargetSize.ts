export type TargetSizeUnit = "KB" | "MB" | "GB";

export const TARGET_SIZE_SAFETY_MARGIN = 0.06;
export const MAX_TARGET_SIZE_BYTES = 100 * 1024 ** 3;

export const targetSizeMultipliers: Record<TargetSizeUnit, number> = {
  KB: 1024,
  MB: 1024 ** 2,
  GB: 1024 ** 3,
};

export interface TargetBitrateEstimate {
  totalKbps: number;
  videoKbps: number;
  audioKbps: number;
  overheadKbps: number;
}

export type TargetSizeValidationCode =
  | "nonNumeric"
  | "zero"
  | "negative"
  | "tooLarge"
  | "tooSmall";

export function targetSizeToBytes(value: number, unit: TargetSizeUnit) {
  return value * targetSizeMultipliers[unit];
}

export function calculateTargetBitrates(
  targetBytes: number,
  durationSeconds: number,
  audioKbps: number,
  safetyMargin = TARGET_SIZE_SAFETY_MARGIN,
): TargetBitrateEstimate | null {
  if (!Number.isFinite(targetBytes) || targetBytes <= 0 || !Number.isFinite(durationSeconds) || durationSeconds <= 0) return null;
  if (!Number.isFinite(audioKbps) || audioKbps < 0 || safetyMargin < 0.04 || safetyMargin > 0.06) return null;
  const totalKbps = (targetBytes * 8) / durationSeconds / 1000;
  const overheadKbps = totalKbps * safetyMargin;
  return {
    totalKbps,
    videoKbps: Math.max(0, totalKbps - audioKbps - overheadKbps),
    audioKbps,
    overheadKbps,
  };
}

export function minimumUsableVideoBitrateKbps(width: number, height: number) {
  const pixels = Math.max(0, width) * Math.max(0, height);
  if (pixels >= 1920 * 1080) return 500;
  if (pixels >= 1280 * 720) return 300;
  if (pixels >= 640 * 360) return 180;
  return 100;
}

export function validateTargetSize(
  rawValue: string,
  unit: TargetSizeUnit,
  estimates: Array<{ durationSeconds: number; audioKbps: number; width: number; height: number }> = [],
): TargetSizeValidationCode | null {
  if (!rawValue.trim()) return "nonNumeric";
  const value = Number(rawValue);
  if (!Number.isFinite(value)) return "nonNumeric";
  if (value === 0) return "zero";
  if (value < 0) return "negative";
  const targetBytes = targetSizeToBytes(value, unit);
  if (!Number.isFinite(targetBytes) || targetBytes > MAX_TARGET_SIZE_BYTES) return "tooLarge";
  const unusable = estimates.some((input) => {
    const bitrate = calculateTargetBitrates(targetBytes, input.durationSeconds, input.audioKbps);
    return bitrate !== null && bitrate.videoKbps < minimumUsableVideoBitrateKbps(input.width, input.height);
  });
  return unusable ? "tooSmall" : null;
}

export function estimatedReductionPercent(originalBytes: number, targetBytes: number) {
  if (!Number.isFinite(originalBytes) || originalBytes <= 0 || !Number.isFinite(targetBytes) || targetBytes < 0) return 0;
  return Math.max(0, (1 - targetBytes / originalBytes) * 100);
}

export function correctedVideoBitrateKbps(firstVideoKbps: number, targetBytes: number, actualBytes: number) {
  if (![firstVideoKbps, targetBytes, actualBytes].every(Number.isFinite) || firstVideoKbps <= 0 || targetBytes <= 0 || actualBytes <= 0) return null;
  return firstVideoKbps * (targetBytes / actualBytes) * 0.97;
}
