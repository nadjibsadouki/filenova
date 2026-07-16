export type SocialPlatform = "tiktok" | "instagram" | "youtube" | "facebook" | "linkedin" | "x" | "custom";
export type SocialMethod = "fit" | "fill" | "stretch";
export type CropPosition = "center" | "top" | "bottom" | "left" | "right" | "custom";
export type SocialQuality = "smaller" | "balanced" | "high" | "maximum";
export type FitBackground = "black" | "white" | "custom" | "blurred";
export type SafeZone = "none" | "tiktok" | "instagram" | "youtube";

export interface SocialPreset {
  id: string;
  platform: SocialPlatform;
  labelKey: string;
  width: number;
  height: number;
  ratioLabel: string;
}

export const socialPresets: SocialPreset[] = [
  { id: "tiktok-vertical", platform: "tiktok", labelKey: "socialPresetVerticalVideo", width: 1080, height: 1920, ratioLabel: "9:16" },
  { id: "instagram-reel", platform: "instagram", labelKey: "socialPresetReelStory", width: 1080, height: 1920, ratioLabel: "9:16" },
  { id: "instagram-square", platform: "instagram", labelKey: "socialPresetSquarePost", width: 1080, height: 1080, ratioLabel: "1:1" },
  { id: "instagram-portrait", platform: "instagram", labelKey: "socialPresetPortraitPost", width: 1080, height: 1350, ratioLabel: "4:5" },
  { id: "instagram-landscape", platform: "instagram", labelKey: "socialPresetLandscapePost", width: 1080, height: 566, ratioLabel: "1.91:1" },
  { id: "youtube-short", platform: "youtube", labelKey: "socialPresetShort", width: 1080, height: 1920, ratioLabel: "9:16" },
  { id: "youtube-standard", platform: "youtube", labelKey: "socialPresetStandardVideo", width: 1920, height: 1080, ratioLabel: "16:9" },
  { id: "facebook-reel", platform: "facebook", labelKey: "socialPresetReelStory", width: 1080, height: 1920, ratioLabel: "9:16" },
  { id: "facebook-feed", platform: "facebook", labelKey: "socialPresetFeedVideo", width: 1280, height: 720, ratioLabel: "16:9" },
  { id: "facebook-square", platform: "facebook", labelKey: "socialPresetSquareVideo", width: 1080, height: 1080, ratioLabel: "1:1" },
  { id: "linkedin-landscape", platform: "linkedin", labelKey: "socialPresetLandscapeVideo", width: 1920, height: 1080, ratioLabel: "16:9" },
  { id: "linkedin-square", platform: "linkedin", labelKey: "socialPresetSquareVideo", width: 1080, height: 1080, ratioLabel: "1:1" },
  { id: "linkedin-vertical", platform: "linkedin", labelKey: "socialPresetVerticalVideo", width: 1080, height: 1350, ratioLabel: "4:5" },
  { id: "x-landscape", platform: "x", labelKey: "socialPresetLandscapeVideo", width: 1280, height: 720, ratioLabel: "16:9" },
  { id: "x-square", platform: "x", labelKey: "socialPresetSquareVideo", width: 1080, height: 1080, ratioLabel: "1:1" },
];

export function ratioLabel(width: number, height: number) {
  if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) return "—";
  const divisor = greatestCommonDivisor(Math.round(width), Math.round(height));
  const left = Math.round(width) / divisor;
  const right = Math.round(height) / divisor;
  return left <= 100 && right <= 100 ? `${left}:${right}` : (width / height).toFixed(2) + ":1";
}

function greatestCommonDivisor(first: number, second: number): number {
  let a = Math.abs(first);
  let b = Math.abs(second);
  while (b) [a, b] = [b, a % b];
  return a || 1;
}

export function plannedCrop(sourceWidth: number, sourceHeight: number, targetWidth: number, targetHeight: number, method: SocialMethod) {
  if ([sourceWidth, sourceHeight, targetWidth, targetHeight].some((value) => !Number.isFinite(value) || value <= 0)) return null;
  const sourceRatio = sourceWidth / sourceHeight;
  const targetRatio = targetWidth / targetHeight;
  if (method !== "fill" || Math.abs(sourceRatio - targetRatio) < 0.0001) return { axis: "none" as const, percent: 0 };
  if (sourceRatio > targetRatio) {
    return { axis: "horizontal" as const, percent: Math.max(0, (1 - targetRatio / sourceRatio) * 100) };
  }
  return { axis: "vertical" as const, percent: Math.max(0, (1 - sourceRatio / targetRatio) * 100) };
}

export function customObjectPosition(position: CropPosition, x: number, y: number) {
  const fixed: Record<Exclude<CropPosition, "custom">, string> = {
    center: "50% 50%", top: "50% 0%", bottom: "50% 100%", left: "0% 50%", right: "100% 50%",
  };
  return position === "custom"
    ? `${Math.min(100, Math.max(0, x))}% ${Math.min(100, Math.max(0, y))}%`
    : fixed[position];
}

export function socialConfigurationValid(width: number, height: number, frameRate: number, preserveFrameRate: boolean, outputName: string) {
  return Number.isFinite(width) && width > 0 && Number.isFinite(height) && height > 0
    && (preserveFrameRate || (Number.isFinite(frameRate) && frameRate > 0 && frameRate <= 120))
    && !!outputName.trim() && !outputName.includes("/") && !outputName.includes("\\");
}
