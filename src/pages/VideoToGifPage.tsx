import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath as openSystemPath, revealItemInDir as revealSystemItem } from "@tauri-apps/plugin-opener";
import {
  AlertCircle,
  Ban,
  Check,
  CheckCircle2,
  Clock3,
  FileVideo2,
  Film,
  FolderOpen,
  Images,
  Play,
  RotateCcw,
  Settings2,
  Trash2,
  UploadCloud,
  X,
} from "lucide-react";
import { appendProcessingHistoryEntry, createVideoProcessingHistoryEntry } from "../history";
import { useI18n, type TranslationKey } from "../i18n";
import type { VideoToolRuntimeChange } from "../navigation";
import { useSettings } from "../settings";
import {
  calculateGifDimensions,
  estimateGif,
  formatGifTime,
  parseGifTime,
  validateGifSelection,
  type GifDimension,
  type GifDithering,
  type GifQuality,
} from "../utils/videoGif";

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
}

interface GifResult {
  outputPath: string;
  outputName: string;
  startSeconds: number;
  endSeconds: number;
  durationSeconds: number;
  width: number;
  height: number;
  framesPerSecond: number;
  finalSize: number;
  processingDurationSeconds: number;
  profile: GifQuality;
  optimizeColors: boolean;
  dithering: GifDithering;
  playCount: number | null;
  frameCount: number;
  estimatedSize: number;
}

interface GifProgress {
  phase: "palette" | "gif";
  phasePercent: number;
  overallPercent: number;
  processedSeconds: number;
  durationSeconds: number;
}

type FrameRateOption = "5" | "10" | "15" | "20" | "24" | "30" | "custom";
type LoopOption = "infinite" | "1" | "2" | "3" | "custom";
const extensions = ["mp4", "mov", "mkv", "avi", "webm", "m4v"];

function formatBytes(bytes: number) {
  const units = ["B", "KB", "MB", "GB"];
  const unit = Math.min(Math.floor(Math.log(Math.max(bytes, 1)) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** unit;
  return `${value >= 10 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
}

function defaultGifName(name: string) {
  return `${name.replace(/\.[^.]+$/, "") || "video"}-animation.gif`;
}

function errorMessage(reason: unknown) {
  if (reason && typeof reason === "object" && "message" in reason) return String(reason.message);
  return String(reason);
}

function gifErrorCode(reason: unknown) {
  return reason && typeof reason === "object" && "code" in reason ? String(reason.code) : "gif_creation_failed";
}

function gifErrorKey(reason: unknown): TranslationKey | null {
  const code = reason && typeof reason === "object" && "code" in reason ? String(reason.code) : "";
  const keys: Record<string, TranslationKey> = {
    invalid_time: "gifInvalidTime", negative_start: "gifStartNegative", start_after_end: "gifStartBeforeEnd",
    end_after_duration: "gifEndAfterDuration", selection_too_short: "gifSelectionTooShort", selection_too_long: "gifSelectionTooLong",
    invalid_dimensions: "gifInvalidDimensions", invalid_frame_rate: "gifInvalidFrameRate", invalid_loop: "gifInvalidLoop",
    ffmpeg_unavailable: "gifFfmpegUnavailable", ffprobe_unavailable: "gifFfprobeUnavailable",
    palette_generation_failed: "gifPaletteFailed", gif_creation_failed: "gifEncodingFailed",
    destination_inaccessible: "gifDestinationFailed", disk_full: "gifDiskFull", gif_cancelled: "gifCancelled",
    output_invalid: "gifOutputInvalid", gif_busy: "gifBusy", cancel_failed: "gifCancelFailed",
  };
  return keys[code] ?? null;
}

interface VideoPageProps { active?: boolean; onRuntimeChange?: VideoToolRuntimeChange }

export function VideoToGifPage({ active = true, onRuntimeChange }: VideoPageProps) {
  const { t, language, setLanguage } = useI18n();
  const { settings } = useSettings();
  const [video, setVideo] = useState<VideoInfo | null>(null);
  const [status, setStatus] = useState<"idle" | "analyzing" | "ready" | "failed">("idle");
  const [inspectionError, setInspectionError] = useState("");
  const [start, setStart] = useState(0);
  const [end, setEnd] = useState(0);
  const [startInput, setStartInput] = useState("00:00:00.000");
  const [endInput, setEndInput] = useState("00:00:00.000");
  const [timeInputError, setTimeInputError] = useState<TranslationKey | null>(null);
  const [currentTime, setCurrentTime] = useState(0);
  const [dimension, setDimension] = useState<GifDimension>("480");
  const [customWidth, setCustomWidth] = useState(480);
  const [frameRate, setFrameRate] = useState<FrameRateOption>("15");
  const [customFrameRate, setCustomFrameRate] = useState(15);
  const [quality, setQuality] = useState<GifQuality>("balanced");
  const [loop, setLoop] = useState<LoopOption>("infinite");
  const [customLoop, setCustomLoop] = useState(1);
  const [keepAspectRatio, setKeepAspectRatio] = useState(true);
  const [optimizeColors, setOptimizeColors] = useState(true);
  const [dithering, setDithering] = useState<GifDithering>("bayer");
  const [destination, setDestination] = useState<string | null>(null);
  const [outputName, setOutputName] = useState("");
  const [isCreating, setIsCreating] = useState(false);
  const [creationError, setCreationError] = useState("");
  const [result, setResult] = useState<GifResult | null>(null);
  const [progress, setProgress] = useState<GifProgress | null>(null);
  const [previewResult, setPreviewResult] = useState(false);
  const player = useRef<HTMLVideoElement>(null);
  const previewing = useRef(false);

  const loadVideo = useCallback(async (paths: string[]) => {
    const path = paths.find((value) => extensions.includes(value.split(".").pop()?.toLowerCase() ?? ""));
    if (!path) return;
    setStatus("analyzing");
    setInspectionError("");
    previewing.current = false;
    try {
      const info = await invoke<VideoInfo>("inspect_video", { path });
      await invoke("allow_video_gif_preview", { path });
      const initialEnd = Math.min(info.durationSeconds, 30);
      setVideo(info);
      setStart(0);
      setEnd(initialEnd);
      setStartInput(formatGifTime(0));
      setEndInput(formatGifTime(initialEnd));
      setCurrentTime(0);
      setOutputName(defaultGifName(info.name));
      setResult(null);
      setCreationError("");
      setStatus("ready");
    } catch (reason) {
      setVideo(null);
      setStatus("failed");
      setInspectionError(String(reason));
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    let disposed = false;
    let cleanup: (() => void) | undefined;
    getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "drop") void loadVideo(event.payload.paths.slice(0, 1));
    }).then((unlisten) => {
      if (disposed) unlisten(); else cleanup = unlisten;
    });
    return () => { disposed = true; cleanup?.(); };
  }, [active, loadVideo]);

  useEffect(() => {
    let disposed = false;
    let cleanup: (() => void) | undefined;
    listen<GifProgress>("video-gif-progress", (event) => {
      if (!disposed) setProgress(event.payload);
    }).then((unlisten) => {
      if (disposed) unlisten(); else cleanup = unlisten;
    });
    return () => { disposed = true; cleanup?.(); };
  }, []);

  const selectedDuration = Math.max(0, end - start);
  const selectionError = useMemo<TranslationKey | null>(() => {
    if (!video) return null;
    return timeInputError ?? validateGifSelection(start, end, video.durationSeconds);
  }, [end, start, timeInputError, video]);
  const selectedFrameRate = frameRate === "custom" ? customFrameRate : Number(frameRate);
  const dimensions = video
    ? calculateGifDimensions(video.width, video.height, dimension, customWidth, keepAspectRatio)
    : { width: 0, height: 0, reduced: false };
  const estimate = estimateGif({
    durationSeconds: selectedDuration,
    width: dimensions.width,
    height: dimensions.height,
    framesPerSecond: selectedFrameRate,
    quality,
    optimizeColors,
    dithering,
  });
  const dimensionInvalid = dimensions.width < 2 || dimensions.height < 2 || (dimension === "custom" && (!Number.isFinite(customWidth) || customWidth <= 0));
  const frameRateInvalid = !Number.isFinite(selectedFrameRate) || selectedFrameRate <= 0 || selectedFrameRate > 120;
  const loopInvalid = loop === "custom" && (!Number.isInteger(customLoop) || customLoop <= 0 || customLoop > 10_000);
  const selectedPlayCount = loop === "infinite" ? null : loop === "custom" ? customLoop : Number(loop);
  const nameInvalid = !outputName.trim() || outputName.includes("/") || outputName.includes("\\");
  const validPlan = !selectionError && !dimensionInvalid && !frameRateInvalid && !loopInvalid && !nameInvalid;
  const canAttemptCreation = !!video && !isCreating;
  const startPercent = video?.durationSeconds ? start / video.durationSeconds * 100 : 0;
  const endPercent = video?.durationSeconds ? end / video.durationSeconds * 100 : 100;

  async function chooseVideo() {
    const selected = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "MP4, MOV, MKV, AVI, WebM, M4V", extensions }],
    });
    if (typeof selected === "string") await loadVideo([selected]);
  }

  function setBoundary(kind: "start" | "end", value: number) {
    const safe = Math.max(0, Math.min(value, video?.durationSeconds ?? value));
    if (kind === "start") {
      setStart(safe);
      setStartInput(formatGifTime(safe));
    } else {
      setEnd(safe);
      setEndInput(formatGifTime(safe));
    }
    setTimeInputError(null);
  }

  function applyTimeInput(kind: "start" | "end", value: string) {
    if (kind === "start") setStartInput(value); else setEndInput(value);
    const parsed = parseGifTime(value);
    if (parsed === null) {
      setTimeInputError("gifInvalidTime");
      return;
    }
    setTimeInputError(null);
    if (kind === "start") setStart(parsed); else setEnd(parsed);
  }

  function previewSelection() {
    if (selectionError || !player.current) return;
    previewing.current = true;
    player.current.currentTime = start;
    void player.current.play();
  }

  function resetSelection() {
    if (!video) return;
    previewing.current = false;
    setBoundary("start", 0);
    setBoundary("end", Math.min(video.durationSeconds, 30));
    if (player.current) player.current.currentTime = 0;
  }

  async function createGif() {
    if (!video || isCreating) return;
    const details = [
      `Dimensions: ${dimensions.width}x${dimensions.height}`,
      `Frame rate: ${selectedFrameRate} fps`,
      `Start: ${formatGifTime(start)}`,
      `Duration: ${formatGifTime(selectedDuration)}`,
    ].join(" · ");
    if (!validPlan) {
      const validationError = selectionError
        ?? (dimensionInvalid ? "gifInvalidDimensions"
          : frameRateInvalid ? "gifInvalidFrameRate"
            : loopInvalid ? "gifInvalidLoop"
              : "gifInvalidName");
      setCreationError(t(validationError));
      void appendProcessingHistoryEntry(createVideoProcessingHistoryEntry({
        tool: "Video to GIF",
        sourceFileName: video.name,
        success: false,
        originalSize: video.size,
        durationMs: 0,
        errorMessage: validationError,
        details,
      }), settings.enableProcessingHistory);
      return;
    }
    const startedAt = Date.now();
    setIsCreating(true);
    setCreationError("");
    setResult(null);
    setProgress(null);
    try {
      const created = await invoke<GifResult>("create_video_gif", {
        request: {
          path: video.path,
          destinationDir: destination,
          outputName,
          startSeconds: start,
          endSeconds: end,
          dimension,
          customWidth: dimension === "custom" ? customWidth : null,
          keepAspectRatio,
          framesPerSecond: selectedFrameRate,
          quality,
          optimizeColors,
          dithering,
          playCount: selectedPlayCount,
        },
      });
      setResult({ ...created, estimatedSize: estimate.estimatedBytes });
      void appendProcessingHistoryEntry(createVideoProcessingHistoryEntry({
        tool: "Video to GIF",
        sourceFileName: video.name,
        resultPath: created.outputPath,
        success: true,
        originalSize: video.size,
        finalSize: created.finalSize,
        durationMs: created.processingDurationSeconds * 1_000,
        details: [
          `Dimensions: ${created.width}x${created.height}`,
          `Frame rate: ${created.framesPerSecond} fps`,
          `Start: ${formatGifTime(created.startSeconds)}`,
          `Duration: ${formatGifTime(created.durationSeconds)}`,
        ].join(" · "),
      }), settings.enableProcessingHistory);
    } catch (reason) {
      const key = gifErrorKey(reason);
      setCreationError(key ? t(key) : errorMessage(reason));
      const code = gifErrorCode(reason);
      void appendProcessingHistoryEntry(createVideoProcessingHistoryEntry({
        tool: "Video to GIF",
        sourceFileName: video.name,
        success: false,
        cancelled: code === "gif_cancelled",
        originalSize: video.size,
        durationMs: Date.now() - startedAt,
        errorMessage: errorMessage(reason) || code,
        details,
      }), settings.enableProcessingHistory);
    } finally {
      setIsCreating(false);
      setProgress(null);
    }
  }

  async function cancelGif() {
    try {
      await invoke("cancel_video_gif");
    } catch (reason) {
      const key = gifErrorKey(reason);
      setCreationError(key ? t(key) : errorMessage(reason));
    }
  }

  useEffect(() => { onRuntimeChange?.("video-to-gif", isCreating, cancelGif); }, [isCreating, onRuntimeChange]);

  async function openPath(path: string) {
    try {
      await openSystemPath(path);
    } catch {
      try {
        await revealSystemItem(path);
        setCreationError(t("gifOpenFallback"));
      } catch {
        setCreationError(t("gifOpenFailed"));
      }
    }
  }

  async function revealItemInDir(path: string) {
    try {
      await revealSystemItem(path);
    } catch {
      setCreationError(t("gifFolderOpenFailed"));
    }
  }

  return <div className="page video-gif-page">
    <header className="page-header compression-header video-header">
      <div><span className="eyebrow">{t("videoGifModuleLabel")}</span><h1>{t("videoGifTitle")}</h1><p>{t("videoGifSubtitle")}</p></div>
      <div className="compress-header-actions"><label className="language-select"><span>{language.toUpperCase()}</span><select value={language} onChange={(event) => setLanguage(event.target.value as "en" | "fr" | "ar")}><option value="en">English</option><option value="fr">Français</option><option value="ar">العربية</option></select></label><div className="header-status"><Film size={17} /><span>FFprobe</span></div></div>
    </header>

    <section className="video-drop-zone video-gif-drop" onClick={() => void chooseVideo()}><div className="video-drop-icon"><UploadCloud size={34} /></div><div><h2>{t("videoGifDropTitle")}</h2><p>{t("videoGifDropHint")}</p></div><button className="primary-button" type="button" onClick={(event) => { event.stopPropagation(); void chooseVideo(); }}><FileVideo2 size={17} />{t("videoGifAdd")}</button></section>
    {status === "analyzing" && <section className="compress-panel video-gif-state"><Clock3 className="spin" size={24} /><strong>{t("analyzingVideo")}</strong></section>}
    {status === "failed" && <section className="compress-panel video-gif-state error"><AlertCircle size={24} /><div><strong>{t("videoInspectionFailed")}</strong><small>{inspectionError}</small></div></section>}

    {video && status === "ready" && <>
      <section className="compress-panel video-gif-source"><div className="compress-panel-header"><div><h2>{video.name}</h2><span title={video.path}>{video.path}</span></div><button className="text-button danger" type="button" onClick={() => { setVideo(null); setStatus("idle"); }}><Trash2 size={14} />{t("remove")}</button></div><div className="gif-source-metadata"><span><small>{t("videoDuration")}</small><strong>{formatGifTime(video.durationSeconds)}</strong></span><span><small>{t("fileSize")}</small><strong>{formatBytes(video.size)}</strong></span><span><small>{t("resolution")}</small><strong>{video.width} × {video.height}</strong></span><span><small>{t("convertVideoFrameRate")}</small><strong>{video.framesPerSecond.toFixed(3)} fps</strong></span><span><small>{t("videoCodec")}</small><strong>{video.videoCodec}</strong></span></div></section>

      <section className="compress-panel gif-player"><video ref={player} src={convertFileSrc(video.path)} controls preload="metadata" onTimeUpdate={(event) => { const element = event.currentTarget; if (previewing.current && element.currentTime < start) element.currentTime = start; if (previewing.current && element.currentTime >= end) { previewing.current = false; element.pause(); element.currentTime = end; } setCurrentTime(element.currentTime); }} onEnded={() => { previewing.current = false; }} /></section>

      <section className="compress-panel gif-timeline-panel"><div className="video-section-title"><Clock3 size={18} /><div><h2>{t("gifTimeSelection")}</h2><p>{t("gifTimeSelectionHint")}</p></div></div><div className="trim-timeline"><div className="trim-timeline-track" /><div className="trim-timeline-selection" style={{ left: `${startPercent}%`, width: `${Math.max(0, endPercent - startPercent)}%` }} /><input aria-label={t("trimStartTime")} className="trim-range trim-range-start" type="range" min="0" max={video.durationSeconds} step="0.001" value={start} onChange={(event) => setBoundary("start", Number(event.target.value))} /><input aria-label={t("trimEndTime")} className="trim-range trim-range-end" type="range" min="0" max={video.durationSeconds} step="0.001" value={end} onChange={(event) => setBoundary("end", Number(event.target.value))} /></div><div className="trim-permanent-times"><span><small>{t("trimStartTime")}</small><strong>{formatGifTime(start)}</strong></span><span><small>{t("trimSelectedDuration")}</small><strong>{formatGifTime(selectedDuration)}</strong></span><span><small>{t("trimEndTime")}</small><strong>{formatGifTime(end)}</strong></span></div><div className="trim-time-fields"><label><span>{t("trimStartTime")}</span><input value={startInput} onChange={(event) => applyTimeInput("start", event.target.value)} /></label><label><span>{t("trimEndTime")}</span><input value={endInput} onChange={(event) => applyTimeInput("end", event.target.value)} /></label></div>{selectionError && <p className="convert-validation"><AlertCircle size={13} />{t(selectionError)}</p>}{selectedDuration > 15 && selectedDuration <= 30 && <p className="gif-duration-warning"><AlertCircle size={14} />{t("gifLongDurationWarning")}</p>}<div className="trim-timeline-actions"><button className="secondary-button" type="button" onClick={() => setBoundary("start", currentTime)}>{t("trimSetStartCurrent")}</button><button className="secondary-button" type="button" onClick={() => setBoundary("end", currentTime)}>{t("trimSetEndCurrent")}</button><button className="text-button" type="button" onClick={resetSelection}><RotateCcw size={14} />{t("trimReset")}</button><button className="primary-button" disabled={!!selectionError} type="button" onClick={previewSelection}><Play size={14} />{t("trimPreviewSelection")}</button></div></section>

      <div className="gif-settings-grid">
        <section className="compress-panel gif-option-panel"><div className="video-section-title"><Settings2 size={18} /><div><h2>{t("gifDimensions")}</h2><p>{t("gifDimensionsHint")}</p></div></div><label className="convert-select"><span>{t("gifWidth")}</span><select value={dimension} onChange={(event) => setDimension(event.target.value as GifDimension)}><option value="original">{t("gifOriginalDimensions")}</option><option value="1080">1080 px</option><option value="720">720 px</option><option value="480">480 px</option><option value="360">360 px</option><option value="custom">{t("gifCustomWidth")}</option></select></label>{dimension === "custom" && <label className="convert-custom-single"><span>{t("gifCustomWidth")}</span><input type="number" min="2" step="2" value={customWidth} onChange={(event) => setCustomWidth(Number(event.target.value))} /></label>}<label className="gif-check"><input type="checkbox" checked={keepAspectRatio} onChange={(event) => setKeepAspectRatio(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><strong>{t("gifKeepAspectRatio")}</strong></label>{dimensions.reduced && <small className="gif-option-note">{t("gifNoUpscaleNote")}</small>}{dimensionInvalid && <p className="convert-validation"><AlertCircle size={13} />{t("gifInvalidDimensions")}</p>}</section>

        <section className="compress-panel gif-option-panel"><div className="video-section-title"><Film size={18} /><div><h2>{t("gifFrameRate")}</h2><p>{t("gifFrameRateHint")}</p></div></div><label className="convert-select"><span>{t("gifFrameRate")}</span><select value={frameRate} onChange={(event) => setFrameRate(event.target.value as FrameRateOption)}>{["5", "10", "15", "20", "24", "30"].map((value) => <option key={value} value={value}>{value} fps</option>)}<option value="custom">{t("gifCustomFrameRate")}</option></select></label>{frameRate === "custom" && <label className="convert-custom-single"><span>{t("gifCustomFrameRate")}</span><input type="number" min="1" max="120" step="1" value={customFrameRate} onChange={(event) => setCustomFrameRate(Number(event.target.value))} /></label>}{frameRateInvalid && <p className="convert-validation"><AlertCircle size={13} />{t("gifInvalidFrameRate")}</p>}</section>

        <section className="compress-panel gif-option-panel"><div className="video-section-title"><Images size={18} /><div><h2>{t("gifQuality")}</h2><p>{t("gifQualityHint")}</p></div></div><div className="gif-choice-buttons">{(["small", "balanced", "high", "maximum"] as GifQuality[]).map((value) => <button type="button" className={quality === value ? "active" : ""} key={value} onClick={() => setQuality(value)}>{t(value === "small" ? "gifQualitySmall" : value === "balanced" ? "gifQualityBalanced" : value === "high" ? "gifQualityHigh" : "gifQualityMaximum")}</button>)}</div><label className="gif-check"><input type="checkbox" checked={optimizeColors} onChange={(event) => setOptimizeColors(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><strong>{t("gifOptimizeColors")}</strong></label>{!optimizeColors && <p className="gif-duration-warning"><AlertCircle size={14} />{t("gifOptimizeColorsDisabledHint")}</p>}<label className="convert-select"><span>{t("gifDithering")}</span><select value={dithering} disabled={!optimizeColors} onChange={(event) => setDithering(event.target.value as GifDithering)}><option value="none">{t("gifDitherNone")}</option><option value="bayer">Bayer</option><option value="floydSteinberg">Floyd-Steinberg</option></select></label></section>

        <section className="compress-panel gif-option-panel"><div className="video-section-title"><RotateCcw size={18} /><div><h2>{t("gifLoop")}</h2><p>{t("gifLoopHint")}</p></div></div><label className="convert-select"><span>{t("gifLoop")}</span><select value={loop} onChange={(event) => setLoop(event.target.value as LoopOption)}><option value="infinite">{t("gifLoopInfinite")}</option><option value="1">{t("gifLoopOnce")}</option><option value="2">{t("gifLoopTwice")}</option><option value="3">{t("gifLoopThree")}</option><option value="custom">{t("gifLoopCustom")}</option></select></label>{loop === "custom" && <label className="convert-custom-single"><span>{t("gifLoopCustom")}</span><input type="number" min="1" step="1" value={customLoop} onChange={(event) => setCustomLoop(Number(event.target.value))} /></label>}{loopInvalid && <p className="convert-validation"><AlertCircle size={13} />{t("gifInvalidLoop")}</p>}</section>
      </div>

      <section className="compress-panel gif-estimate"><div className="video-section-title"><Images size={18} /><div><h2>{t("gifEstimateTitle")}</h2><p>{t("gifEstimateHint")}</p></div></div><div className="gif-estimate-grid"><span><small>{t("gifEstimatedDuration")}</small><strong>{formatGifTime(selectedDuration)}</strong></span><span><small>{t("gifEstimatedDimensions")}</small><strong>{dimensions.width} × {dimensions.height}</strong></span><span><small>{t("gifEstimatedFrameRate")}</small><strong>{selectedFrameRate} fps</strong></span><span><small>{t("gifEstimatedFrames")}</small><strong>≈ {estimate.approximateFrames}</strong></span><span><small>{t("gifEstimatedSize")}</small><strong>≈ {formatBytes(estimate.estimatedBytes)}</strong></span></div><p className="gif-estimate-disclaimer"><AlertCircle size={13} />{t("gifEstimateDisclaimer")}</p></section>

      <section className="compress-panel video-destination-panel"><div><FolderOpen size={19} /><span><strong>{t("destinationFolder")}</strong><small title={destination ?? ""}>{destination ?? t("videoAutomaticDestination")}</small></span></div><button className="secondary-button" type="button" onClick={async () => { const selected = await open({ directory: true, multiple: false }); if (typeof selected === "string") setDestination(selected); }}>{t("chooseFolder")}</button></section><label className="compress-panel trim-output-name"><span>{t("gifOutputName")}</span><input value={outputName} onChange={(event) => setOutputName(event.target.value)} /></label>{nameInvalid && <p className="convert-validation gif-name-error"><AlertCircle size={13} />{t("gifInvalidName")}</p>}
      {progress && <section className="video-progress-panel gif-progress-panel"><div><span>{t(progress.phase === "palette" ? "gifGeneratingPalette" : "gifCreatingPhase")}</span><strong>{t("gifCurrentStep")}: {progress.phase === "palette" ? "1 / 2" : "2 / 2"}</strong><b>{progress.overallPercent.toFixed(1)}%</b></div><div className="gif-progress-bars"><small>{t("gifPhaseProgress")} · {progress.phasePercent.toFixed(1)}%</small><progress max="100" value={progress.phasePercent} /><small>{t("gifOverallProgress")} · {progress.overallPercent.toFixed(1)}%</small><progress max="100" value={progress.overallPercent} /></div><small>{t("gifProcessedTime")}: {formatGifTime(progress.processedSeconds)} / {formatGifTime(progress.durationSeconds)}</small></section>}
      {creationError && <p className="convert-validation gif-name-error"><AlertCircle size={13} />{creationError}</p>}
      {result && <section className="compress-panel trim-result"><div className="convert-result-mode"><CheckCircle2 size={14} />{t("gifCreationComplete")}</div><div className="convert-result-grid"><span><small>{t("gifEstimatedDuration")}</small><strong>{formatGifTime(result.durationSeconds)}</strong></span><span><small>{t("gifEstimatedDimensions")}</small><strong>{result.width} × {result.height}</strong></span><span><small>{t("gifEstimatedFrameRate")}</small><strong>{result.framesPerSecond} fps</strong></span><span><small>{t("gifAppliedProfile")}</small><strong>{t(result.profile === "small" ? "gifQualitySmall" : result.profile === "balanced" ? "gifQualityBalanced" : result.profile === "high" ? "gifQualityHigh" : "gifQualityMaximum")}</strong></span><span><small>{t("gifOptimizeColors")}</small><strong>{t(result.optimizeColors ? "enabled" : "disabled")}</strong></span><span><small>{t("gifDithering")}</small><strong>{result.optimizeColors ? result.dithering === "none" ? t("gifDitherNone") : result.dithering === "bayer" ? "Bayer" : "Floyd-Steinberg" : "—"}</strong></span><span><small>{t("convertFinalSize")}</small><strong>{formatBytes(result.finalSize)}</strong></span><span><small>{t("processingDuration")}</small><strong>{formatGifTime(result.processingDurationSeconds)}</strong></span></div><div className="video-result-location"><FolderOpen size={13} /><span title={result.outputPath}>{result.outputPath}</span></div><div className="video-result-actions"><button className="text-button" type="button" onClick={() => void openPath(result.outputPath)}><Images size={13} />{t("gifOpen")}</button><button className="text-button" type="button" onClick={() => void revealItemInDir(result.outputPath)}><FolderOpen size={13} />{t("openFolder")}</button></div></section>}
      {result && <p className="gif-estimate-disclaimer">{t("gifLoopResult")}: <strong>{result.playCount === null ? t("gifLoopInfinite") : result.playCount === 1 ? t("gifLoopOnce") : `${result.playCount} ${t("gifPlays")}`}</strong> · {t("gifFrameCount")}: <strong>{result.frameCount}</strong></p>}
      {result && <section className="compress-panel gif-final-summary"><div className="convert-result-grid"><span><small>{t("trimRequestedStart")}</small><strong>{formatGifTime(result.startSeconds)}</strong></span><span><small>{t("trimRequestedEnd")}</small><strong>{formatGifTime(result.endSeconds)}</strong></span><span><small>{t("gifEstimatedFrames")}</small><strong>≈ {Math.ceil(result.durationSeconds * result.framesPerSecond)}</strong></span><span><small>{t("gifEstimatedSize")}</small><strong>≈ {formatBytes(result.estimatedSize)}</strong></span></div><div className="video-result-actions"><button className="text-button" type="button" onClick={() => setPreviewResult(true)}><Play size={13} />{t("gifPreview")}</button><button className="text-button" type="button" onClick={() => void openPath(result.outputPath)}><Images size={13} />{t("gifOpen")}</button><button className="text-button" type="button" onClick={() => void revealItemInDir(result.outputPath)}><FolderOpen size={13} />{t("openFolder")}</button></div></section>}
      {isCreating ? <button className="run-button video-cancel-button gif-create-button" type="button" onClick={() => void cancelGif()}><Ban size={19} />{t("gifCancel")}</button> : <button className="run-button gif-create-button" type="button" disabled={!canAttemptCreation} title={!validPlan ? t("gifFixValidation") : undefined} onClick={() => void createGif()}><Images size={19} />{t("gifCreateAction")}</button>}
      {previewResult && result && <div className="video-preview-backdrop" role="presentation" onMouseDown={(event) => { if (event.currentTarget === event.target) setPreviewResult(false); }}><section className="pdf-image-preview-modal" role="dialog" aria-modal="true" aria-label={t("gifPreview")}><header><div><strong>{result.outputName}</strong><small>{result.width} × {result.height} · {result.framesPerSecond} fps</small></div><button className="icon-button" type="button" onClick={() => setPreviewResult(false)}><X size={17} /></button></header><div><img src={convertFileSrc(result.outputPath)} alt={result.outputName} /></div></section></div>}
    </>}
  </div>;
}
