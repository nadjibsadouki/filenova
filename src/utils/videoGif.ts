export type GifDimension = "original" | "1080" | "720" | "480" | "360" | "custom";
export type GifQuality = "small" | "balanced" | "high" | "maximum";
export type GifDithering = "none" | "bayer" | "floydSteinberg";

export interface GifEstimateInput {
  durationSeconds: number;
  width: number;
  height: number;
  framesPerSecond: number;
  quality: GifQuality;
  optimizeColors: boolean;
  dithering: GifDithering;
}

export function formatGifTime(seconds: number) {
  const safe = Math.max(0, Number.isFinite(seconds) ? seconds : 0);
  const totalMilliseconds = Math.round(safe * 1_000);
  const hours = Math.floor(totalMilliseconds / 3_600_000);
  const minutes = Math.floor((totalMilliseconds % 3_600_000) / 60_000);
  const wholeSeconds = Math.floor((totalMilliseconds % 60_000) / 1_000);
  const milliseconds = totalMilliseconds % 1_000;
  return `${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}:${String(wholeSeconds).padStart(2, "0")}.${String(milliseconds).padStart(3, "0")}`;
}

export function parseGifTime(value: string) {
  const match = /^(\d{2}):([0-5]\d):([0-5]\d)(?:\.(\d{1,3}))?$/.exec(value.trim());
  if (!match) return null;
  const fraction = match[4] ? Number(match[4].padEnd(3, "0")) / 1_000 : 0;
  return Number(match[1]) * 3_600 + Number(match[2]) * 60 + Number(match[3]) + fraction;
}

export function evenDimension(value: number) {
  if (!Number.isFinite(value) || value < 2) return 0;
  return Math.max(2, Math.floor(value / 2) * 2);
}

export function calculateGifDimensions(
  sourceWidth: number,
  sourceHeight: number,
  dimension: GifDimension,
  customWidth: number,
  keepAspectRatio: boolean,
) {
  if (sourceWidth < 2 || sourceHeight < 2) return { width: 0, height: 0, reduced: false };
  const requested = dimension === "original"
    ? sourceWidth
    : dimension === "custom"
      ? customWidth
      : Number(dimension);
  if (!Number.isFinite(requested) || requested <= 0) return { width: 0, height: 0, reduced: false };
  const width = evenDimension(Math.min(requested, sourceWidth));
  const height = keepAspectRatio
    ? evenDimension(sourceHeight * width / sourceWidth)
    : evenDimension(sourceHeight);
  return { width, height, reduced: width < evenDimension(sourceWidth) };
}

export function estimateGif(input: GifEstimateInput) {
  const duration = Math.max(0, input.durationSeconds);
  const fps = Math.max(0, input.framesPerSecond);
  const approximateFrames = Math.ceil(duration * fps);
  const qualityFactors: Record<GifQuality, number> = {
    small: 0.035,
    balanced: 0.06,
    high: 0.09,
    maximum: 0.13,
  };
  const ditheringFactors: Record<GifDithering, number> = {
    none: 0.82,
    bayer: 0.96,
    floydSteinberg: 1.06,
  };
  const colorFactor = input.optimizeColors ? 0.76 : 1;
  const estimatedBytes = Math.max(
    0,
    Math.round(input.width * input.height * approximateFrames * qualityFactors[input.quality] * ditheringFactors[input.dithering] * colorFactor),
  );
  return { approximateFrames, estimatedBytes };
}

export function validateGifSelection(start: number, end: number, totalDuration: number) {
  if (!Number.isFinite(start) || !Number.isFinite(end)) return "gifInvalidTime" as const;
  if (start < 0) return "gifStartNegative" as const;
  if (start >= end) return "gifStartBeforeEnd" as const;
  if (end > totalDuration + 0.0005) return "gifEndAfterDuration" as const;
  if (end - start < 0.0995) return "gifSelectionTooShort" as const;
  if (end - start > 30.0005) return "gifSelectionTooLong" as const;
  return null;
}
