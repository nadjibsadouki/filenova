import { useCallback, useEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { AlertTriangle, Ban, Check, CheckCircle2, Clapperboard, Crop, FileVideo2, FolderOpen, Grid3X3, Play, Settings2, Trash2, UploadCloud, Video, X } from "lucide-react";
import { useI18n, type TranslationKey } from "../i18n";
import type { VideoToolRuntimeChange } from "../navigation";
import {
  customObjectPosition,
  plannedCrop,
  ratioLabel,
  socialConfigurationValid,
  socialPresets,
  type CropPosition,
  type FitBackground,
  type SafeZone,
  type SocialMethod,
  type SocialPlatform,
  type SocialQuality,
} from "../utils/socialMedia";

interface VideoInfo {
  path: string;
  name: string;
  format: string;
  durationSeconds: number;
  width: number;
  height: number;
  framesPerSecond: number;
  size: number;
  videoCodec: string;
  audioCodec: string | null;
  thumbnailData: number[] | null;
}

interface SocialExportResult {
  outputPath: string;
  outputName: string;
  width: number;
  height: number;
  framesPerSecond: number;
  videoCodec: string;
  audioCodec: string | null;
  finalSize: number;
  processingDurationSeconds: number;
}

interface SocialExportProgress {
  processedSeconds: number;
  durationSeconds: number;
  percent: number;
  phase: string;
}

interface SocialExportSummary extends SocialExportResult {
  platform: SocialPlatform;
  presetId: string;
  method: SocialMethod;
  background: FitBackground;
  backgroundColor: string;
  cropPosition: CropPosition;
  cropX: number;
  cropY: number;
  originalWidth: number;
  originalHeight: number;
  originalSize: number;
  quality: SocialQuality;
}

const extensions = ["mp4", "mov", "mkv", "avi", "webm", "m4v"];
const platforms: SocialPlatform[] = ["tiktok", "instagram", "youtube", "facebook", "linkedin", "x", "custom"];

function bytes(value: number) {
  const units = ["B", "KB", "MB", "GB"];
  const index = Math.min(Math.floor(Math.log(Math.max(1, value)) / Math.log(1024)), units.length - 1);
  return `${(value / 1024 ** index).toFixed(index ? 1 : 0)} ${units[index]}`;
}

function duration(value: number) {
  const seconds = Math.max(0, Math.round(value));
  return `${String(Math.floor(seconds / 60)).padStart(2, "0")}:${String(seconds % 60).padStart(2, "0")}`;
}

function defaultName(name: string, platform: SocialPlatform, presetId: string) {
  const stem = name.replace(/\.[^.]+$/, "") || "video";
  return `${stem}-${platform === "custom" ? "social" : presetId}.mp4`;
}

function platformKey(platform: SocialPlatform): TranslationKey {
  const keys: Record<SocialPlatform, TranslationKey> = {
    tiktok: "socialTikTok", instagram: "socialInstagram", youtube: "socialYouTube", facebook: "socialFacebook",
    linkedin: "socialLinkedIn", x: "socialX", custom: "socialCustom",
  };
  return keys[platform];
}

interface VideoPageProps { active?: boolean; onRuntimeChange?: VideoToolRuntimeChange }

export function SocialMediaToolsPage({ active = true, onRuntimeChange }: VideoPageProps) {
  const { t, language, setLanguage } = useI18n();
  const [video, setVideo] = useState<VideoInfo | null>(null);
  const [status, setStatus] = useState<"idle" | "analyzing" | "ready" | "failed">("idle");
  const [error, setError] = useState("");
  const [thumbnail, setThumbnail] = useState<string | null>(null);
  const [platform, setPlatform] = useState<SocialPlatform>("tiktok");
  const [presetId, setPresetId] = useState("tiktok-vertical");
  const [customWidth, setCustomWidth] = useState(1080);
  const [customHeight, setCustomHeight] = useState(1920);
  const [customRatio, setCustomRatio] = useState("9:16");
  const [method, setMethod] = useState<SocialMethod>("fit");
  const [stretchConfirmed, setStretchConfirmed] = useState(false);
  const [background, setBackground] = useState<FitBackground>("black");
  const [backgroundColor, setBackgroundColor] = useState("#24332c");
  const [cropPosition, setCropPosition] = useState<CropPosition>("center");
  const [cropX, setCropX] = useState(50);
  const [cropY, setCropY] = useState(50);
  const [safeZone, setSafeZone] = useState<SafeZone>("none");
  const [preserveFrameRate, setPreserveFrameRate] = useState(true);
  const [customFrameRate, setCustomFrameRate] = useState(30);
  const [preserveAudio, setPreserveAudio] = useState(true);
  const [preserveMetadata, setPreserveMetadata] = useState(false);
  const [quality, setQuality] = useState<SocialQuality>("balanced");
  const [destination, setDestination] = useState<string | null>(null);
  const [outputName, setOutputName] = useState("");
  const [isExporting, setIsExporting] = useState(false);
  const [exportError, setExportError] = useState("");
  const [exportResult, setExportResult] = useState<SocialExportSummary | null>(null);
  const [exportProgress, setExportProgress] = useState<SocialExportProgress | null>(null);
  const [previewResult, setPreviewResult] = useState(false);

  const loadVideo = useCallback(async (paths: string[]) => {
    const path = paths.find((candidate) => extensions.includes(candidate.split(".").pop()?.toLowerCase() ?? ""));
    if (!path || isExporting) return;
    setStatus("analyzing");
    setError("");
    try {
      const info = await invoke<VideoInfo>("inspect_video", { path });
      setVideo(info);
      setCustomFrameRate(Math.max(1, Math.round(info.framesPerSecond)));
      setStatus("ready");
      setThumbnail((current) => {
        if (current) URL.revokeObjectURL(current);
        return info.thumbnailData?.length
          ? URL.createObjectURL(new Blob([new Uint8Array(info.thumbnailData)], { type: "image/png" }))
          : null;
      });
    } catch (reason) {
      setVideo(null);
      setStatus("failed");
      setError(String(reason));
    }
  }, [isExporting]);

  useEffect(() => {
    if (video) setOutputName(defaultName(video.name, platform, presetId));
  }, [video?.path, platform, presetId]);

  useEffect(() => {
    if (!active) return;
    let disposed = false;
    let cleanup: (() => void) | undefined;
    listen<SocialExportProgress>("social-media-export-progress", (event) => {
      if (!disposed) setExportProgress(event.payload);
    }).then((unlisten) => { if (disposed) unlisten(); else cleanup = unlisten; });
    return () => { disposed = true; cleanup?.(); };
  }, []);

  useEffect(() => () => { if (thumbnail) URL.revokeObjectURL(thumbnail); }, [thumbnail]);
  useEffect(() => {
    let disposed = false;
    let cleanup: (() => void) | undefined;
    getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "drop") void loadVideo(event.payload.paths.slice(0, 1));
    }).then((unlisten) => { if (disposed) unlisten(); else cleanup = unlisten; });
    return () => { disposed = true; cleanup?.(); };
  }, [active, loadVideo]);

  const availablePresets = socialPresets.filter((preset) => preset.platform === platform);
  const selectedPreset = socialPresets.find((preset) => preset.id === presetId);
  const targetWidth = platform === "custom" ? customWidth : selectedPreset?.width ?? 0;
  const targetHeight = platform === "custom" ? customHeight : selectedPreset?.height ?? 0;
  const targetRatio = platform === "custom" ? customRatio : selectedPreset?.ratioLabel ?? "—";
  const customRatioValid = platform !== "custom" || /^\s*\d+(?:\.\d+)?\s*:\s*\d+(?:\.\d+)?\s*$/.test(customRatio);
  const valid = !!video && customRatioValid && socialConfigurationValid(targetWidth, targetHeight, customFrameRate, preserveFrameRate, outputName);
  const crop = video ? plannedCrop(video.width, video.height, targetWidth, targetHeight, method) : null;
  const upscaling = !!video && (targetWidth > video.width || targetHeight > video.height);
  const objectPosition = customObjectPosition(cropPosition, cropX, cropY);
  const plannedFrameRate = preserveFrameRate ? video?.framesPerSecond ?? 0 : customFrameRate;
  const stretchReady = method !== "stretch" || stretchConfirmed;
  const baseExportReady = valid && stretchReady;
  const previewBackground = background === "white" ? "#fff" : background === "custom" ? backgroundColor : "#101412";

  async function chooseVideo() {
    if (isExporting) return;
    const selected = await open({ multiple: false, directory: false, filters: [{ name: "MP4, MOV, MKV, AVI, WebM, M4V", extensions }] });
    if (typeof selected === "string") await loadVideo([selected]);
  }

  function choosePlatform(value: SocialPlatform) {
    setPlatform(value);
    const first = socialPresets.find((preset) => preset.platform === value);
    if (first) setPresetId(first.id);
    if (value === "tiktok") setSafeZone("tiktok");
    else if (value === "instagram") setSafeZone("instagram");
    else if (value === "youtube") setSafeZone("youtube");
  }

  async function exportVideo() {
    if (!video || !baseExportReady || isExporting) return;
    setIsExporting(true);
    setExportError("");
    setExportResult(null);
    setPreviewResult(false);
    setExportProgress({ processedSeconds: 0, durationSeconds: video.durationSeconds, percent: 0, phase: "preparing" });
    try {
      const result = await invoke<SocialExportResult>("export_social_media_video", {
        request: {
          path: video.path,
          destinationDir: destination,
          outputName,
          targetWidth,
          targetHeight,
          method,
          stretchConfirmed,
          background,
          customColor: background === "custom" ? backgroundColor : null,
          cropPosition,
          cropX,
          cropY,
          preserveFrameRate,
          framesPerSecond: customFrameRate,
          preserveAudio,
          preserveMetadata,
          quality,
        },
      });
      setExportResult({
        ...result,
        platform,
        presetId,
        method,
        background,
        backgroundColor,
        cropPosition,
        cropX,
        cropY,
        originalWidth: video.width,
        originalHeight: video.height,
        originalSize: video.size,
        quality,
      });
    } catch (reason) {
      const code = reason && typeof reason === "object" && "code" in reason ? String(reason.code) : "";
      const keys: Record<string, TranslationKey> = {
        social_export_busy: "socialExportBusy", ffmpeg_unavailable: "socialFfmpegUnavailable",
        ffprobe_unavailable: "socialFfprobeUnavailable", social_encoder_unavailable: "socialEncoderUnavailable",
        invalid_social_dimensions: "socialInvalidDimensions", invalid_social_frame_rate: "socialInvalidFrameRate",
        stretch_confirmation_required: "socialStretchConfirmationRequired",
        invalid_social_quality: "socialInvalidQuality", social_export_cancelled: "socialExportCancelled",
        social_cancel_failed: "socialCancelFailed",
        invalid_social_background: "socialInvalidBackground", invalid_crop_position: "socialInvalidCropPosition",
        destination_inaccessible: "socialDestinationFailed", disk_full: "socialDiskFull",
        social_output_invalid: "socialOutputInvalid", social_export_failed: "socialExportFailed",
        invalid_output_name: "socialInvalidOutputName",
      };
      const key = keys[code];
      setExportError(key ? t(key) : String(reason));
    } finally {
      setIsExporting(false);
      setExportProgress(null);
    }
  }

  async function cancelExport() {
    try {
      await invoke<boolean>("cancel_social_media_export");
    } catch (reason) {
      const message = reason && typeof reason === "object" && "message" in reason ? String(reason.message) : String(reason);
      setExportError(message || t("socialCancelFailed"));
    }
  }

  useEffect(() => { onRuntimeChange?.("social-media-tools", isExporting, cancelExport); }, [isExporting, onRuntimeChange]);

  async function openExportedVideo(path: string) {
    try {
      await openPath(path);
    } catch {
      setExportError(t("socialOpenVideoFailed"));
    }
  }

  async function revealExportedVideo(path: string) {
    try {
      await revealItemInDir(path);
    } catch {
      setExportError(t("socialOpenFolderFailed"));
    }
  }

  function backgroundOrCrop(result: SocialExportSummary) {
    if (result.method === "stretch") return t("socialStretchPlan");
    if (result.method === "fit") {
      if (result.background === "custom") return `${t("backgroundCustom")} · ${result.backgroundColor.toUpperCase()}`;
      return t(result.background === "black" ? "backgroundBlack" : result.background === "white" ? "backgroundWhite" : "socialBlurredBackground");
    }
    if (result.cropPosition === "custom") return `${t("socialCustomPosition")} · X ${result.cropX}% · Y ${result.cropY}%`;
    return t(result.cropPosition === "center" ? "socialCenter" : result.cropPosition === "top" ? "socialTop" : result.cropPosition === "bottom" ? "socialBottom" : result.cropPosition === "left" ? "socialLeft" : "socialRight");
  }

  function qualityLabel(value: SocialQuality): TranslationKey {
    return value === "smaller" ? "socialQualitySmaller" : value === "balanced" ? "gifQualityBalanced" : value === "high" ? "gifQualityHigh" : "gifQualityMaximum";
  }

  const cropDescription = method === "fit" ? t("socialNoCrop") : method === "stretch" ? t("socialStretchPlan")
    : crop?.axis === "horizontal" ? t("socialHorizontalCrop", { percent: crop.percent.toFixed(1) })
      : crop?.axis === "vertical" ? t("socialVerticalCrop", { percent: crop.percent.toFixed(1) }) : t("socialNoCrop");
  const resultPreset = exportResult ? socialPresets.find((preset) => preset.id === exportResult.presetId) : null;

  return <div className="page social-media-page">
    <header className="page-header compression-header video-header">
      <div><span className="eyebrow">{t("socialModuleLabel")}</span><h1>{t("socialTitle")}</h1><p>{t("socialSubtitle")}</p></div>
      <div className="compress-header-actions"><label className="language-select"><span>{language.toUpperCase()}</span><select value={language} onChange={(event) => setLanguage(event.target.value as "en" | "fr" | "ar")}><option value="en">English</option><option value="fr">Français</option><option value="ar">العربية</option></select></label><div className="header-status"><Clapperboard size={17}/><span>FFprobe</span></div></div>
    </header>

    <section className="video-drop-zone social-drop" onClick={() => void chooseVideo()}><div className="video-drop-icon"><UploadCloud size={34}/></div><div><h2>{t("socialDropTitle")}</h2><p>{t("socialDropHint")}</p></div><button className="primary-button" type="button" onClick={(event) => { event.stopPropagation(); void chooseVideo(); }}><FileVideo2 size={17}/>{t("socialAddVideo")}</button></section>
    {status === "analyzing" && <section className="compress-panel social-state"><strong>{t("analyzingVideo")}</strong></section>}
    {status === "failed" && <section className="compress-panel social-state error"><strong>{t("videoInspectionFailed")}</strong><small>{error}</small></section>}

    {video && status === "ready" && <>
      <section className="compress-panel social-source"><div className="compress-panel-header"><div><h2>{video.name}</h2><span title={video.path}>{video.path}</span></div><button className="text-button danger" type="button" onClick={() => { setVideo(null); setStatus("idle"); }}><Trash2 size={14}/>{t("remove")}</button></div><div className="social-metadata"><span><small>{t("videoDuration")}</small><strong>{duration(video.durationSeconds)}</strong></span><span><small>{t("fileSize")}</small><strong>{bytes(video.size)}</strong></span><span><small>{t("resolution")}</small><strong>{video.width} × {video.height}</strong></span><span><small>{t("socialOrientation")}</small><strong>{t(video.height > video.width ? "socialVertical" : video.width > video.height ? "socialHorizontal" : "socialSquare")}</strong></span><span><small>{t("convertVideoFrameRate")}</small><strong>{video.framesPerSecond.toFixed(3)} fps</strong></span><span><small>{t("videoCodec")}</small><strong>{video.videoCodec}</strong></span><span><small>{t("audioCodec")}</small><strong>{video.audioCodec ?? t("socialNone")}</strong></span></div></section>

      <section className="compress-panel social-preset-panel"><div className="video-section-title"><Grid3X3 size={18}/><div><h2>{t("socialPlatformsPresets")}</h2><p>{t("socialPlatformsHint")}</p></div></div><div className="social-platform-tabs">{platforms.map((value) => <button type="button" key={value} className={platform === value ? "active" : ""} onClick={() => choosePlatform(value)}>{t(platformKey(value))}</button>)}</div>{platform !== "custom" ? <div className="social-preset-grid">{availablePresets.map((preset) => <button type="button" key={preset.id} className={presetId === preset.id ? "active" : ""} onClick={() => setPresetId(preset.id)}><strong>{t(preset.labelKey as TranslationKey)}</strong><small>{preset.width} × {preset.height} · {preset.ratioLabel}</small></button>)}</div> : <div className="social-custom-grid"><label><span>{t("width")}</span><input type="number" min="1" value={customWidth} onChange={(event) => setCustomWidth(Number(event.target.value))}/></label><label><span>{t("height")}</span><input type="number" min="1" value={customHeight} onChange={(event) => setCustomHeight(Number(event.target.value))}/></label><label><span>{t("socialCustomRatio")}</span><input dir="ltr" value={customRatio} onChange={(event) => setCustomRatio(event.target.value)}/></label></div>}</section>

      <div className="social-workspace-grid">
        <section className="compress-panel social-settings"><div className="video-section-title"><Crop size={18}/><div><h2>{t("socialAdaptationMethod")}</h2><p>{t("socialAdaptationHint")}</p></div></div><div className="social-methods">{(["fit", "fill", "stretch"] as SocialMethod[]).map((value) => <button type="button" key={value} className={method === value ? "active" : ""} onClick={() => { setMethod(value); if (value === "stretch") setStretchConfirmed(false); }}><strong>{t(value === "fit" ? "fitMethod" : value === "fill" ? "fillMethod" : "stretchMethod")}</strong><small>{t(value === "fit" ? "socialFitHint" : value === "fill" ? "socialFillHint" : "socialStretchHint")}</small></button>)}</div>{method === "stretch" && <><p className="social-warning"><AlertTriangle size={14}/>{t("socialStretchWarning")}</p><label className="gif-check social-stretch-confirm"><input type="checkbox" checked={stretchConfirmed} onChange={(event) => setStretchConfirmed(event.target.checked)}/><span className="custom-check"><Check size={13}/></span><strong>{t("socialConfirmStretch")}</strong></label></>}
          {method === "fit" && <div className="social-subsettings"><strong>{t("socialFitBackground")}</strong><div className="social-inline-options">{(["black", "white", "custom", "blurred"] as FitBackground[]).map((value) => <button type="button" className={background === value ? "active" : ""} key={value} onClick={() => setBackground(value)}>{t(value === "black" ? "backgroundBlack" : value === "white" ? "backgroundWhite" : value === "custom" ? "backgroundCustom" : "socialBlurredBackground")}</button>)}</div>{background === "custom" && <input aria-label={t("customColor")} type="color" value={backgroundColor} onChange={(event) => setBackgroundColor(event.target.value)}/>} {background === "blurred" && <small>{t("socialBlurredPreviewOnly")}</small>}</div>}
          {method === "fill" && <div className="social-subsettings"><strong>{t("socialCropPosition")}</strong><div className="social-inline-options">{(["center", "top", "bottom", "left", "right", "custom"] as CropPosition[]).map((value) => <button type="button" className={cropPosition === value ? "active" : ""} key={value} onClick={() => setCropPosition(value)}>{t(value === "center" ? "socialCenter" : value === "top" ? "socialTop" : value === "bottom" ? "socialBottom" : value === "left" ? "socialLeft" : value === "right" ? "socialRight" : "socialCustomPosition")}</button>)}</div>{cropPosition === "custom" && <div className="social-position-fields"><label>X<input type="range" min="0" max="100" value={cropX} onChange={(event) => setCropX(Number(event.target.value))}/><b>{cropX}%</b></label><label>Y<input type="range" min="0" max="100" value={cropY} onChange={(event) => setCropY(Number(event.target.value))}/><b>{cropY}%</b></label></div>}</div>}
        </section>

        <section className="compress-panel social-preview-panel"><div className="video-section-title"><Crop size={18}/><div><h2>{t("socialFramingPreview")}</h2><p>{t("socialPreviewHint")}</p></div></div><div className="social-preview-stage" style={{ aspectRatio: `${targetWidth || 1} / ${targetHeight || 1}`, background: previewBackground }}>{background === "blurred" && thumbnail && <img className="social-preview-blur" src={thumbnail} alt=""/>}{thumbnail ? <img className="social-preview-image" src={thumbnail} alt={video.name} style={{ objectFit: method === "fit" ? "contain" : method === "fill" ? "cover" : "fill", objectPosition }}/>: <FileVideo2 size={42}/>} {safeZone !== "none" && <div className={`social-safe-zone ${safeZone}`}><span>{t(safeZone === "tiktok" ? "socialTikTokSafeZone" : safeZone === "instagram" ? "socialInstagramSafeZone" : "socialYouTubeSafeZone")}</span></div>}</div><label className="convert-select"><span>{t("socialSafeZones")}</span><select value={safeZone} onChange={(event) => setSafeZone(event.target.value as SafeZone)}><option value="none">{t("socialNone")}</option><option value="tiktok">{t("socialTikTokSafeZone")}</option><option value="instagram">{t("socialInstagramSafeZone")}</option><option value="youtube">{t("socialYouTubeSafeZone")}</option></select></label><small className="social-preview-note">{t("socialSafeZonePreviewOnly")}</small></section>
      </div>

      <section className="compress-panel social-options"><div className="video-section-title"><Settings2 size={18}/><div><h2>{t("socialOutputOptions")}</h2><p>{t("socialOutputOptionsHint")}</p></div></div><div className="social-option-grid"><div><label className="gif-check"><input type="checkbox" checked={preserveFrameRate} onChange={(event) => setPreserveFrameRate(event.target.checked)}/><span className="custom-check"><Check size={13}/></span><strong>{t("socialPreserveFrameRate")}</strong></label>{!preserveFrameRate && <label className="convert-custom-single"><span>{t("socialCustomFrameRate")}</span><input type="number" min="1" max="120" value={customFrameRate} onChange={(event) => setCustomFrameRate(Number(event.target.value))}/></label>}</div><label className="gif-check"><input type="checkbox" checked={preserveAudio} onChange={(event) => setPreserveAudio(event.target.checked)}/><span className="custom-check"><Check size={13}/></span><strong>{t("socialPreserveAudio")}</strong></label><label className="gif-check"><input type="checkbox" checked={preserveMetadata} onChange={(event) => setPreserveMetadata(event.target.checked)}/><span className="custom-check"><Check size={13}/></span><strong>{t("socialPreserveMetadata")}</strong></label><label className="convert-select"><span>{t("socialVideoQuality")}</span><select value={quality} onChange={(event) => setQuality(event.target.value as SocialQuality)}><option value="smaller">{t("socialQualitySmaller")}</option><option value="balanced">{t("gifQualityBalanced")}</option><option value="high">{t("gifQualityHigh")}</option><option value="maximum">{t("gifQualityMaximum")}</option></select></label></div></section>

      <section className="compress-panel social-export-plan"><div className="video-section-title"><Clapperboard size={18}/><div><h2>{t("socialExportPlan")}</h2><p>{t("socialExportPlanHint")}</p></div></div><div className="social-plan-grid"><span><small>{t("socialPlatform")}</small><strong>{t(platformKey(platform))}</strong></span><span><small>{t("socialPreset")}</small><strong>{platform === "custom" ? t("socialCustom") : t((selectedPreset?.labelKey ?? "socialCustom") as TranslationKey)}</strong></span><span><small>{t("socialOriginalDimensions")}</small><strong>{video.width} × {video.height}</strong></span><span><small>{t("socialFinalDimensions")}</small><strong>{targetWidth} × {targetHeight}</strong></span><span><small>{t("socialOriginalRatio")}</small><strong>{ratioLabel(video.width, video.height)}</strong></span><span><small>{t("socialFinalRatio")}</small><strong>{targetRatio}</strong></span><span><small>{t("socialMethod")}</small><strong>{t(method === "fit" ? "fitMethod" : method === "fill" ? "fillMethod" : "stretchMethod")}</strong></span><span><small>{t("socialPlannedCrop")}</small><strong>{cropDescription}</strong></span><span><small>{t("socialPlannedFrameRate")}</small><strong>{plannedFrameRate.toFixed(3)} fps</strong></span><span><small>{t("socialPlannedCodec")}</small><strong>H.264{preserveAudio && video.audioCodec ? " / AAC" : ""}</strong></span></div><p className="social-reencode"><AlertTriangle size={14}/>{t("socialReencodeRequired")}</p>{upscaling && <p className="social-warning"><AlertTriangle size={14}/>{t("socialUpscaleWarning")}</p>}</section>

      <section className="compress-panel video-destination-panel social-destination"><div><small>{t("destinationFolder")}</small><strong title={destination ?? ""}>{destination ?? t("sameFolder")}</strong></div><button className="secondary-button" type="button" onClick={async () => { const selected = await open({ directory: true, multiple: false }); if (typeof selected === "string") setDestination(selected); }}><FolderOpen size={14}/>{t("chooseFolder")}</button></section><label className="compress-panel trim-output-name social-output-name"><span>{t("socialOutputName")}</span><input value={outputName} onChange={(event) => setOutputName(event.target.value)}/></label>
      {!valid && <p className="convert-validation social-validation"><AlertTriangle size={13}/>{t("socialInvalidConfiguration")}</p>}
      {valid && method === "stretch" && !stretchConfirmed && <p className="convert-validation social-validation"><AlertTriangle size={13}/>{t("socialStretchConfirmationRequired")}</p>}
      {exportError && <p className="convert-validation social-validation"><AlertTriangle size={13}/>{exportError}</p>}
      {exportResult && <section className="compress-panel social-export-result"><div className="convert-result-mode"><CheckCircle2 size={14}/>{t("socialExportComplete")}</div><div className="social-plan-grid"><span><small>{t("socialPlatform")}</small><strong className="social-result-text">{t(platformKey(exportResult.platform))}</strong></span><span><small>{t("socialPreset")}</small><strong className="social-result-text">{exportResult.platform === "custom" ? t("socialCustom") : t((resultPreset?.labelKey ?? "socialCustom") as TranslationKey)}</strong></span><span><small>{t("socialMethod")}</small><strong className="social-result-text">{t(exportResult.method === "fit" ? "fitMethod" : exportResult.method === "fill" ? "fillMethod" : "stretchMethod")}</strong></span><span><small>{t(exportResult.method === "fit" ? "socialAppliedBackground" : exportResult.method === "fill" ? "socialAppliedCropPosition" : "socialAdaptationDetail")}</small><strong className="social-result-text">{backgroundOrCrop(exportResult)}</strong></span><span><small>{t("socialOriginalDimensions")}</small><strong>{exportResult.originalWidth} × {exportResult.originalHeight}</strong></span><span><small>{t("socialFinalDimensions")}</small><strong>{exportResult.width} × {exportResult.height}</strong></span><span><small>{t("socialFinalRatio")}</small><strong>{ratioLabel(exportResult.width, exportResult.height)}</strong></span><span><small>{t("socialFinalFrameRate")}</small><strong>{exportResult.framesPerSecond.toFixed(3)} fps</strong></span><span><small>{t("videoCodec")}</small><strong>{exportResult.videoCodec}</strong></span><span><small>{t("audioCodec")}</small><strong>{exportResult.audioCodec ?? t("noAudio")}</strong></span><span><small>{t("socialVideoQuality")}</small><strong className="social-result-text">{t(qualityLabel(exportResult.quality))}</strong></span><span><small>{t("originalSize")}</small><strong>{bytes(exportResult.originalSize)}</strong></span><span><small>{t("convertFinalSize")}</small><strong>{bytes(exportResult.finalSize)}</strong></span><span><small>{t("processingDuration")}</small><strong>{exportResult.processingDurationSeconds.toFixed(2)} s</strong></span></div><div className="video-result-location"><FolderOpen size={13}/><span title={exportResult.outputPath}>{exportResult.outputPath}</span></div><div className="video-result-actions"><button className="text-button" type="button" onClick={() => setPreviewResult(true)}><Play size={13}/>{t("previewVideo")}</button><button className="text-button" type="button" onClick={() => void openExportedVideo(exportResult.outputPath)}><Video size={13}/>{t("openVideo")}</button><button className="text-button" type="button" onClick={() => void revealExportedVideo(exportResult.outputPath)}><FolderOpen size={13}/>{t("openFolder")}</button></div></section>}
      {exportProgress && <section className="video-progress-panel social-progress"><div><span>{t("socialPreparingVideo")}</span><strong>{video.name}</strong><b>{exportProgress.percent.toFixed(1)}%</b></div><progress max="100" value={exportProgress.percent}/><small>{duration(exportProgress.processedSeconds)} / {duration(exportProgress.durationSeconds)}</small></section>}
      {isExporting ? <button className="run-button video-cancel-button social-export-button" type="button" onClick={() => void cancelExport()}><Ban size={19}/>{t("socialCancelExport")}</button> : <button className="run-button social-export-button" type="button" disabled={!baseExportReady} title={method === "stretch" && !stretchConfirmed ? t("socialStretchConfirmationRequired") : undefined} onClick={() => void exportVideo()}><Clapperboard size={19}/>{t("socialExportVideo")}</button>}
    </>}
    {previewResult && exportResult && <div className="video-preview-backdrop" role="presentation" onMouseDown={(event) => { if (event.currentTarget === event.target) setPreviewResult(false); }}><section className="video-preview-modal" role="dialog" aria-modal="true" aria-label={t("previewVideo")}><header><div><strong>{exportResult.outputName}</strong><small>{exportResult.width} × {exportResult.height} · {exportResult.framesPerSecond.toFixed(3)} fps · {exportResult.videoCodec}</small></div><button className="icon-button" type="button" onClick={() => setPreviewResult(false)} aria-label={t("closePreview")}><X size={17}/></button></header><video src={convertFileSrc(exportResult.outputPath)} controls autoPlay/></section></div>}
  </div>;
}
