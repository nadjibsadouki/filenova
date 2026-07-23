import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  AlertCircle,
  Ban,
  Check,
  CheckCircle2,
  Clapperboard,
  Clock3,
  FileVideo2,
  FolderOpen,
  Pause,
  Play,
  RotateCcw,
  Scissors,
  Settings2,
  Trash2,
  UploadCloud,
  Video,
  X,
} from "lucide-react";
import { appendProcessingHistoryEntry, createVideoProcessingHistoryEntry } from "../history";
import { useI18n, type TranslationKey } from "../i18n";
import type { VideoToolRuntimeChange } from "../navigation";
import { useSettings } from "../settings";

type OutputFormat = "mp4" | "mov" | "mkv" | "webm";
type CutMode = "fast" | "precise";

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
}

interface FastTrimProgress {
  path: string;
  name: string;
  processedSeconds: number;
  durationSeconds: number;
  percent: number;
}

interface FastTrimResult {
  requestedStartSeconds: number;
  requestedEndSeconds: number;
  requestedDurationSeconds: number;
  detectedActualStartSeconds: number;
  finalDurationSeconds: number;
  keyframeOffsetSeconds: number;
  finalFormat: string;
  finalVideoCodec: string;
  finalAudioCodec: string | null;
  originalSize: number;
  finalSize: number;
  processingDurationSeconds: number;
  outputPath: string;
  outputName: string;
}

interface PreciseTrimResult {
  method: string;
  requestedStartSeconds: number;
  requestedEndSeconds: number;
  requestedDurationSeconds: number;
  finalDurationSeconds: number;
  durationDifferenceSeconds: number;
  finalFormat: string;
  finalVideoCodec: string;
  finalAudioCodec: string | null;
  quality: number;
  width: number;
  height: number;
  framesPerSecond: number;
  originalSize: number;
  finalSize: number;
  processingDurationSeconds: number;
  outputPath: string;
  outputName: string;
}

const extensions = ["mp4", "mov", "mkv", "avi", "webm", "m4v"];
const fastTrimErrors: Record<string, TranslationKey> = {
  invalid_trim_selection: "trimInvalidSelection",
  container_incompatible: "trimContainerIncompatible",
  fast_cut_video_incompatible: "trimFastVideoIncompatible",
  fast_cut_audio_incompatible: "trimFastAudioIncompatible",
  ffmpeg_unavailable: "ffmpegUnavailable",
  ffprobe_unavailable: "ffprobeUnavailable",
  destination_inaccessible: "videoDestinationInaccessible",
  disk_full: "videoDiskFull",
  trim_cancelled: "trimCancelled",
  output_empty: "videoOutputEmpty",
  output_invalid: "videoOutputInvalid",
  output_duration_invalid: "trimOutputDurationInvalid",
  invalid_output_name: "trimInvalidOutputName",
  trim_busy: "trimBusy",
  trim_failed: "trimFailed",
  format_unavailable: "trimPreciseFormatUnavailable",
  encoder_unavailable: "trimPreciseEncoderUnavailable",
  invalid_quality: "trimPreciseInvalidQuality",
  output_desynchronized: "trimPreciseDesynchronized",
};

function parseTrimError(error: unknown): { code: string; message: string } {
  if (typeof error === "object" && error !== null) {
    const value = error as { code?: unknown; message?: unknown };
    return {
      code: String(value.code ?? "trim_failed"),
      message: typeof value.message === "string" ? value.message : "",
    };
  }
  try {
    return parseTrimError(JSON.parse(String(error)));
  } catch {
    return { code: "trim_failed", message: String(error) };
  }
}

function formatBytes(bytes: number) {
  const units = ["B", "KB", "MB", "GB"];
  const unit = Math.min(Math.floor(Math.log(Math.max(bytes, 1)) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** unit;
  return `${value >= 10 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
}

export function formatTrimTime(seconds: number, milliseconds = true) {
  const safe = Math.max(0, Number.isFinite(seconds) ? seconds : 0);
  const totalMilliseconds = Math.round(safe * 1_000);
  const hours = Math.floor(totalMilliseconds / 3_600_000);
  const minutes = Math.floor((totalMilliseconds % 3_600_000) / 60_000);
  const wholeSeconds = Math.floor((totalMilliseconds % 60_000) / 1_000);
  const fraction = totalMilliseconds % 1_000;
  return `${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}:${String(wholeSeconds).padStart(2, "0")}${milliseconds ? `.${String(fraction).padStart(3, "0")}` : ""}`;
}

export function parseTrimTime(value: string) {
  const match = /^(-?)(\d{2}):([0-5]\d):([0-5]\d)(?:\.(\d{1,3}))?$/.exec(value.trim());
  if (!match) return null;
  const fraction = match[5] ? Number(match[5].padEnd(3, "0")) / 1_000 : 0;
  const seconds = Number(match[2]) * 3_600 + Number(match[3]) * 60 + Number(match[4]) + fraction;
  return match[1] ? -seconds : seconds;
}

function trimHistoryDetails(mode: CutMode, start: number, end: number) {
  return [
    `Mode: ${mode === "fast" ? "Fast Cut" : "Precise Cut"}`,
    `Start: ${formatTrimTime(start)}`,
    `End: ${formatTrimTime(end)}`,
    `Selected duration: ${formatTrimTime(Math.max(0, end - start))}`,
  ].join(" · ");
}

function copyCompatible(format: OutputFormat, codec: string) {
  const value = codec.toUpperCase().replace(/-/g, "");
  if (format === "webm") return ["VP8", "VP9", "AV1"].includes(value);
  if (format === "mp4") return ["H264", "HEVC", "AV1", "MPEG4"].includes(value);
  if (format === "mov") return ["H264", "HEVC", "MPEG4", "PRORES"].includes(value);
  return value !== "UNKNOWN";
}

function audioCopyCompatible(format: string, codec: string | null) {
  if (!codec) return true;
  const value = codec.toUpperCase().replace(/-/g, "");
  if (format === "webm") return ["OPUS", "VORBIS"].includes(value);
  if (["mp4", "m4v", "mov"].includes(format)) return ["AAC", "MP3", "ALAC", "OPUS"].includes(value);
  return ["mkv", "avi"].includes(format) && value !== "UNKNOWN";
}

function defaultOutputName(name: string, mode: CutMode = "fast") {
  const stem = name.replace(/\.[^.]+$/, "") || "video";
  return mode === "precise" ? `${stem}-precise-trim` : `${stem}-trimmed`;
}

function ResultActions({ result, t, onPreview }: {
  result: FastTrimResult | PreciseTrimResult;
  t: (key: TranslationKey) => string;
  onPreview: () => void;
}) {
  return <><div className="video-result-location"><FolderOpen size={13} /><span title={result.outputPath}>{result.outputPath}</span></div><div className="video-result-actions"><button className="text-button" type="button" onClick={onPreview}><Play size={13} />{t("previewVideo")}</button><button className="text-button" type="button" onClick={() => void openPath(result.outputPath)}><Video size={13} />{t("openVideo")}</button><button className="text-button" type="button" onClick={() => void revealItemInDir(result.outputPath)}><FolderOpen size={13} />{t("openFolder")}</button></div></>;
}

interface VideoPageProps { active?: boolean; onRuntimeChange?: VideoToolRuntimeChange }

export function TrimVideoPage({ active = true, onRuntimeChange }: VideoPageProps) {
  const { t, language, setLanguage } = useI18n();
  const { settings } = useSettings();
  const [video, setVideo] = useState<VideoInfo | null>(null);
  const [status, setStatus] = useState<"idle" | "analyzing" | "ready" | "failed">("idle");
  const [error, setError] = useState<string | null>(null);
  const [start, setStart] = useState(0);
  const [end, setEnd] = useState(0);
  const [startInput, setStartInput] = useState("00:00:00.000");
  const [endInput, setEndInput] = useState("00:00:00.000");
  const [inputError, setInputError] = useState<TranslationKey | null>(null);
  const [currentTime, setCurrentTime] = useState(0);
  const [isPlaying, setIsPlaying] = useState(false);
  const [mode, setMode] = useState<CutMode>("fast");
  const [preserveFormat, setPreserveFormat] = useState(true);
  const [outputFormat, setOutputFormat] = useState<OutputFormat>("mp4");
  const [preserveMetadata, setPreserveMetadata] = useState(false);
  const [removeAudio, setRemoveAudio] = useState(false);
  const [qualityMode, setQualityMode] = useState<"recommended" | "custom">("recommended");
  const [quality, setQuality] = useState(75);
  const [preciseCodec, setPreciseCodec] = useState<"auto" | "openh264" | "svtAv1">("auto");
  const [destination, setDestination] = useState<string | null>(null);
  const [outputName, setOutputName] = useState("");
  const [isProcessing, setIsProcessing] = useState(false);
  const [progress, setProgress] = useState<FastTrimProgress | null>(null);
  const [result, setResult] = useState<FastTrimResult | PreciseTrimResult | null>(null);
  const [runError, setRunError] = useState<TranslationKey | null>(null);
  const [previewResult, setPreviewResult] = useState(false);
  const player = useRef<HTMLVideoElement>(null);
  const previewing = useRef(false);

  const loadVideo = useCallback(async (paths: string[]) => {
    const path = paths.find((value) => extensions.includes(value.split(".").pop()?.toLowerCase() ?? ""));
    if (!path) return;
    setStatus("analyzing");
    setError(null);
    setRunError(null);
    setResult(null);
    previewing.current = false;
    try {
      const info = await invoke<VideoInfo>("inspect_video", { path });
      await invoke("allow_trim_video_preview", { path });
      setVideo(info);
      setStart(0);
      setEnd(info.durationSeconds);
      setStartInput(formatTrimTime(0));
      setEndInput(formatTrimTime(info.durationSeconds));
      setCurrentTime(0);
      setOutputName(defaultOutputName(info.name));
      setStatus("ready");
    } catch (reason) {
      setVideo(null);
      setStatus("failed");
      setError(String(reason));
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
    listen<FastTrimProgress>("fast-trim-progress", (event) => {
      if (!disposed) setProgress(event.payload);
    }).then((unlisten) => {
      if (disposed) unlisten(); else cleanup = unlisten;
    });
    return () => { disposed = true; cleanup?.(); };
  }, []);

  useEffect(() => {
    let disposed = false;
    let cleanup: (() => void) | undefined;
    listen<FastTrimProgress>("precise-trim-progress", (event) => {
      if (!disposed) setProgress(event.payload);
    }).then((unlisten) => {
      if (disposed) unlisten(); else cleanup = unlisten;
    });
    return () => { disposed = true; cleanup?.(); };
  }, []);

  const selectionError = useMemo<TranslationKey | null>(() => {
    if (!video || inputError) return inputError;
    if (start < 0) return "trimStartNegative";
    if (end > video.durationSeconds + 0.000_5) return "trimEndAfterDuration";
    if (start >= end) return "trimStartBeforeEnd";
    if (end - start < 0.099_5) return "trimSelectionTooShort";
    return null;
  }, [end, inputError, start, video]);
  const selectionValid = !!video && !selectionError;
  const selectedDuration = Math.max(0, end - start);
  const finalFormat = preserveFormat ? video?.format ?? "—" : outputFormat === "webm" ? "WebM" : outputFormat.toUpperCase();
  const targetContainer = preserveFormat ? video?.format.toLowerCase() ?? "" : outputFormat;
  const videoCompatible = !!video && (preserveFormat || copyCompatible(outputFormat, video.videoCodec));
  const audioCompatible = !!video && (removeAudio || audioCopyCompatible(targetContainer, video.audioCodec));
  const fastCompatible = videoCompatible && audioCompatible;
  const noReencoding = !!video && mode === "fast" && fastCompatible;
  const canFastCut = selectionValid && mode === "fast" && fastCompatible && outputName.trim().length > 0 && !isProcessing;
  const canAttemptFastCut = !!video && mode === "fast" && fastCompatible && outputName.trim().length > 0 && !isProcessing;
  const preciseFormatSupported = ["mp4", "mov", "mkv", "webm"].includes(targetContainer);
  const effectiveQuality = qualityMode === "recommended" ? 75 : quality;
  const plannedPreciseVideoCodec = targetContainer === "webm" ? "SVT-AV1" : targetContainer === "mkv" && preciseCodec === "svtAv1" ? "SVT-AV1" : "OpenH264";
  const plannedPreciseAudioCodec = removeAudio || !video?.audioCodec ? t("noAudio") : plannedPreciseVideoCodec === "SVT-AV1" ? "Opus" : "AAC";
  const canPreciseCut = selectionValid && mode === "precise" && preciseFormatSupported && outputName.trim().length > 0 && !isProcessing;
  const canAttemptPreciseCut = !!video && mode === "precise" && preciseFormatSupported && outputName.trim().length > 0 && !isProcessing;
  const startPercent = video?.durationSeconds ? Math.max(0, Math.min(100, start / video.durationSeconds * 100)) : 0;
  const endPercent = video?.durationSeconds ? Math.max(0, Math.min(100, end / video.durationSeconds * 100)) : 100;

  useEffect(() => {
    if (!video) return;
    const fastName = defaultOutputName(video.name, "fast");
    const preciseName = defaultOutputName(video.name, "precise");
    if (outputName === fastName || outputName === preciseName) {
      setOutputName(mode === "precise" ? preciseName : fastName);
    }
  }, [mode, outputName, video]);

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
      setStartInput(formatTrimTime(safe));
    } else {
      setEnd(safe);
      setEndInput(formatTrimTime(safe));
    }
    setInputError(null);
  }

  function applyTimeInput(kind: "start" | "end", value: string) {
    if (kind === "start") setStartInput(value); else setEndInput(value);
    const parsed = parseTrimTime(value);
    if (parsed === null) {
      setInputError("trimInvalidTimeFormat");
      return;
    }
    setInputError(null);
    if (kind === "start") setStart(parsed); else setEnd(parsed);
  }

  function previewSelection() {
    if (!selectionValid || !player.current) return;
    previewing.current = true;
    player.current.currentTime = start;
    void player.current.play();
  }

  function resetSelection() {
    if (!video) return;
    previewing.current = false;
    setBoundary("start", 0);
    setBoundary("end", video.durationSeconds);
    if (player.current) player.current.currentTime = 0;
  }

  async function runFastCut() {
    if (!video || !canAttemptFastCut) return;
    const details = trimHistoryDetails("fast", start, end);
    if (!canFastCut) {
      const code = selectionError ?? "trimInvalidSelection";
      setRunError(selectionError ?? "trimInvalidSelection");
      void appendProcessingHistoryEntry(createVideoProcessingHistoryEntry({
        tool: "Trim Video",
        sourceFileName: video.name,
        success: false,
        originalSize: video.size,
        durationMs: 0,
        errorMessage: code,
        details,
      }), settings.enableProcessingHistory);
      return;
    }
    const startedAt = Date.now();
    setIsProcessing(true);
    setRunError(null);
    setResult(null);
    setProgress({
      path: video.path,
      name: video.name,
      processedSeconds: 0,
      durationSeconds: selectedDuration,
      percent: 0,
    });
    try {
      const value = await invoke<FastTrimResult>("fast_trim_video", {
        request: {
          path: video.path,
          destinationDir: destination,
          preserveOriginalFormat: preserveFormat,
          outputFormat,
          preserveMetadata,
          removeAudio,
          outputName,
          startSeconds: start,
          endSeconds: end,
        },
      });
      setResult(value);
      void appendProcessingHistoryEntry(createVideoProcessingHistoryEntry({
        tool: "Trim Video",
        sourceFileName: video.name,
        resultPath: value.outputPath,
        success: true,
        originalSize: value.originalSize,
        finalSize: value.finalSize,
        durationMs: value.processingDurationSeconds * 1_000,
        details: trimHistoryDetails("fast", value.requestedStartSeconds, value.requestedEndSeconds),
      }), settings.enableProcessingHistory);
    } catch (reason) {
      const parsed = parseTrimError(reason);
      const cancelled = parsed.code === "trim_cancelled";
      setRunError(fastTrimErrors[parsed.code] ?? "trimFailed");
      void appendProcessingHistoryEntry(createVideoProcessingHistoryEntry({
        tool: "Trim Video",
        sourceFileName: video.name,
        success: false,
        cancelled,
        originalSize: video.size,
        durationMs: Date.now() - startedAt,
        errorMessage: parsed.message || parsed.code,
        details,
      }), settings.enableProcessingHistory);
    } finally {
      setProgress(null);
      setIsProcessing(false);
    }
  }

  async function runPreciseCut() {
    if (!video || !canAttemptPreciseCut) return;
    const details = trimHistoryDetails("precise", start, end);
    if (!canPreciseCut) {
      const code = selectionError ?? "trimInvalidSelection";
      setRunError(selectionError ?? "trimInvalidSelection");
      void appendProcessingHistoryEntry(createVideoProcessingHistoryEntry({
        tool: "Trim Video",
        sourceFileName: video.name,
        success: false,
        originalSize: video.size,
        durationMs: 0,
        errorMessage: code,
        details,
      }), settings.enableProcessingHistory);
      return;
    }
    const startedAt = Date.now();
    setIsProcessing(true);
    setRunError(null);
    setResult(null);
    setProgress({ path: video.path, name: video.name, processedSeconds: 0, durationSeconds: selectedDuration, percent: 0 });
    try {
      const value = await invoke<PreciseTrimResult>("precise_trim_video", {
        request: {
          path: video.path,
          destinationDir: destination,
          preserveOriginalFormat: preserveFormat,
          outputFormat,
          preserveMetadata,
          removeAudio,
          outputName,
          startSeconds: start,
          endSeconds: end,
          quality: effectiveQuality,
          videoCodec: preciseCodec,
        },
      });
      setResult(value);
      void appendProcessingHistoryEntry(createVideoProcessingHistoryEntry({
        tool: "Trim Video",
        sourceFileName: video.name,
        resultPath: value.outputPath,
        success: true,
        originalSize: value.originalSize,
        finalSize: value.finalSize,
        durationMs: value.processingDurationSeconds * 1_000,
        details: trimHistoryDetails("precise", value.requestedStartSeconds, value.requestedEndSeconds),
      }), settings.enableProcessingHistory);
    } catch (reason) {
      const parsed = parseTrimError(reason);
      const cancelled = parsed.code === "trim_cancelled";
      setRunError(fastTrimErrors[parsed.code] ?? "trimPreciseFailed");
      void appendProcessingHistoryEntry(createVideoProcessingHistoryEntry({
        tool: "Trim Video",
        sourceFileName: video.name,
        success: false,
        cancelled,
        originalSize: video.size,
        durationMs: Date.now() - startedAt,
        errorMessage: parsed.message || parsed.code,
        details,
      }), settings.enableProcessingHistory);
    } finally {
      setProgress(null);
      setIsProcessing(false);
    }
  }

  async function cancelFastCut() {
    try {
      await invoke("cancel_fast_trim");
    } catch (reason) {
      const parsed = parseTrimError(reason);
      setRunError(fastTrimErrors[parsed.code] ?? "trimCancelFailed");
    }
  }

  async function cancelActiveCut() {
    if (mode === "fast") return cancelFastCut();
    try {
      await invoke("cancel_precise_trim");
    } catch (reason) {
      const parsed = parseTrimError(reason);
      setRunError(fastTrimErrors[parsed.code] ?? "trimCancelFailed");
    }
  }

  useEffect(() => { onRuntimeChange?.("trim-video", isProcessing, cancelActiveCut); }, [isProcessing, onRuntimeChange]);

  return <div className="page trim-video-page">
    <header className="page-header compression-header video-header">
      <div><span className="eyebrow">{t("trimVideoModuleLabel")}</span><h1>{t("trimVideoTitle")}</h1><p>{t("trimVideoSubtitle")}</p></div>
      <div className="compress-header-actions"><label className="language-select"><span>{language.toUpperCase()}</span><select value={language} onChange={(event) => setLanguage(event.target.value as "en" | "fr" | "ar")}><option value="en">English</option><option value="fr">Français</option><option value="ar">العربية</option></select></label><div className="header-status"><Scissors size={17} /><span>FFprobe</span></div></div>
    </header>

    <section className="video-drop-zone trim-video-drop" onClick={() => void chooseVideo()}><div className="video-drop-icon"><UploadCloud size={34} /></div><div><h2>{t("trimVideoDropTitle")}</h2><p>{t("trimVideoDropHint")}</p></div><button className="primary-button" type="button" onClick={(event) => { event.stopPropagation(); void chooseVideo(); }}><FileVideo2 size={17} />{t("trimVideoAdd")}</button></section>

    {status === "analyzing" && <section className="compress-panel trim-video-state"><Clock3 className="spin" size={24} /><strong>{t("analyzingVideo")}</strong></section>}
    {status === "failed" && <section className="compress-panel trim-video-state error"><AlertCircle size={24} /><strong>{t("videoInspectionFailed")}</strong><small>{error}</small></section>}

    {video && status === "ready" && <>
      <section className="compress-panel trim-video-source"><div className="compress-panel-header"><div><h2>{video.name}</h2><span title={video.path}>{video.path}</span></div><button className="text-button danger" type="button" onClick={() => { setVideo(null); setStatus("idle"); }}><Trash2 size={14} />{t("remove")}</button></div><div className="trim-source-metadata"><span><small>{t("videoDuration")}</small><strong>{formatTrimTime(video.durationSeconds)}</strong></span><span><small>{t("fileSize")}</small><strong>{formatBytes(video.size)}</strong></span><span><small>{t("resolution")}</small><strong>{video.width} × {video.height}</strong></span><span><small>{t("convertVideoFrameRate")}</small><strong>{video.framesPerSecond.toFixed(3)} fps</strong></span><span><small>{t("videoCodec")}</small><strong>{video.videoCodec}</strong></span><span><small>{t("audioCodec")}</small><strong>{video.audioCodec ?? t("noAudio")}</strong></span></div></section>

      <section className="compress-panel trim-player-panel"><video ref={player} src={convertFileSrc(video.path)} controls preload="metadata" onPlay={() => setIsPlaying(true)} onPause={() => setIsPlaying(false)} onTimeUpdate={(event) => { const element = event.currentTarget; if (previewing.current && element.currentTime < start) element.currentTime = start; if (previewing.current && element.currentTime >= end) { previewing.current = false; element.pause(); element.currentTime = end; } setCurrentTime(element.currentTime); }} onEnded={() => { previewing.current = false; setIsPlaying(false); }} /><div className="trim-player-status"><button className="icon-button" type="button" onClick={() => { if (!player.current) return; if (player.current.paused) void player.current.play(); else player.current.pause(); }}>{isPlaying ? <Pause size={16} /> : <Play size={16} />}</button><span>{formatTrimTime(currentTime)} / {formatTrimTime(video.durationSeconds)}</span></div></section>

      <section className="compress-panel trim-timeline-panel"><div className="video-section-title"><Clock3 size={18} /><div><h2>{t("trimTimeline")}</h2><p>{t("trimTimelineHint")}</p></div></div><div className="trim-timeline"><div className="trim-timeline-track" /><div className="trim-timeline-selection" style={{ left: `${startPercent}%`, width: `${Math.max(0, endPercent - startPercent)}%` }} /><input aria-label={t("trimStartTime")} className="trim-range trim-range-start" type="range" min="0" max={video.durationSeconds} step="0.001" value={start} onChange={(event) => setBoundary("start", Number(event.target.value))} /><input aria-label={t("trimEndTime")} className="trim-range trim-range-end" type="range" min="0" max={video.durationSeconds} step="0.001" value={end} onChange={(event) => setBoundary("end", Number(event.target.value))} /></div><div className="trim-permanent-times"><span><small>{t("trimStartTime")}</small><strong>{formatTrimTime(start)}</strong></span><span><small>{t("trimSelectedDuration")}</small><strong>{formatTrimTime(selectedDuration)}</strong></span><span><small>{t("trimEndTime")}</small><strong>{formatTrimTime(end)}</strong></span></div>

        <div className="trim-time-fields"><label><span>{t("trimStartTime")}</span><input value={startInput} onChange={(event) => applyTimeInput("start", event.target.value)} /></label><label><span>{t("trimEndTime")}</span><input value={endInput} onChange={(event) => applyTimeInput("end", event.target.value)} /></label></div>{selectionError && <p className="convert-validation"><AlertCircle size={13} />{t(selectionError)}</p>}
        <div className="trim-timeline-actions"><button className="secondary-button" type="button" onClick={() => setBoundary("start", currentTime)}>{t("trimSetStartCurrent")}</button><button className="secondary-button" type="button" onClick={() => setBoundary("end", currentTime)}>{t("trimSetEndCurrent")}</button><button className="text-button" type="button" onClick={resetSelection}><RotateCcw size={14} />{t("trimReset")}</button><button className="primary-button" disabled={!selectionValid} type="button" onClick={previewSelection}><Play size={14} />{t("trimPreviewSelection")}</button></div>
      </section>

      <div className="convert-video-settings-grid trim-settings-grid"><section className="compress-panel trim-mode-panel"><div className="video-section-title"><Scissors size={18} /><div><h2>{t("trimPrecision")}</h2><p>{t("trimPrecisionHint")}</p></div></div><div className="trim-mode-options">{(["fast", "precise"] as CutMode[]).map((value) => <button type="button" disabled={isProcessing} className={mode === value ? "active" : ""} key={value} onClick={() => setMode(value)}><strong>{t(value === "fast" ? "trimFastCut" : "trimPreciseCut")}</strong><small>{t(value === "fast" ? "trimFastCutDescription" : "trimPreciseCutDescription")}</small></button>)}</div></section>
        <section className="compress-panel convert-video-options-panel"><div className="video-section-title"><Settings2 size={18} /><div><h2>{t("videoOptions")}</h2><p>{t("trimOptionsHint")}</p></div></div><div className="convert-checks"><label><input type="checkbox" checked={preserveFormat} onChange={(event) => setPreserveFormat(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><strong>{t("trimPreserveFormat")}</strong></label><label><input type="checkbox" checked={preserveMetadata} onChange={(event) => setPreserveMetadata(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><strong>{t("preserveMetadata")}</strong></label><label><input type="checkbox" checked={removeAudio} onChange={(event) => setRemoveAudio(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><strong>{t("removeAudio")}</strong></label></div><label className="convert-select"><span>{t("convertOutputFormat")}</span><select value={outputFormat} disabled={preserveFormat} onChange={(event) => setOutputFormat(event.target.value as OutputFormat)}><option value="mp4">MP4</option><option value="mov">MOV</option><option value="mkv">MKV</option><option value="webm">WebM</option></select></label>{mode === "precise" && <div className="trim-precise-options"><span className="trim-option-label">{t("trimPreciseQuality")}</span><div className="trim-quality-modes"><button type="button" className={qualityMode === "recommended" ? "active" : ""} onClick={() => setQualityMode("recommended")}>{t("trimQualityRecommended")}</button><button type="button" className={qualityMode === "custom" ? "active" : ""} onClick={() => setQualityMode("custom")}>{t("trimQualityCustom")}</button></div>{qualityMode === "custom" && <label className="trim-quality-slider"><span>{t("trimQualityValue")}</span><input type="range" min="1" max="100" value={quality} onChange={(event) => setQuality(Number(event.target.value))} /><strong>{quality}</strong></label>}{targetContainer === "mkv" && <label className="convert-select"><span>{t("trimPreciseVideoCodec")}</span><select value={preciseCodec} onChange={(event) => setPreciseCodec(event.target.value as "auto" | "openh264" | "svtAv1")}><option value="auto">Auto</option><option value="openh264">OpenH264 / AAC</option><option value="svtAv1">SVT-AV1 / Opus</option></select></label>}<div className="trim-precise-codecs"><span>{plannedPreciseVideoCodec}</span><span>{plannedPreciseAudioCodec}</span><strong>{t("trimPreciseReencoding")}</strong></div></div>}</section></div>

      <section className="compress-panel trim-plan-panel"><div className="video-section-title"><Clapperboard size={18} /><div><h2>{t("trimPlanTitle")}</h2><p>{t("trimPlanHint")}</p></div></div><div className={`convert-plan ${(mode === "fast" && !fastCompatible) || (mode === "precise" && !preciseFormatSupported) ? "invalid" : "valid"}`}><div><span><small>{t("trimStartTime")}</small><strong>{formatTrimTime(start)}</strong></span><span><small>{t("trimEndTime")}</small><strong>{formatTrimTime(end)}</strong></span><span><small>{t("trimSelectedDuration")}</small><strong>{formatTrimTime(selectedDuration)}</strong></span><span><small>{t("convertOriginalFormat")}</small><strong>{video.format}</strong></span><span><small>{t("convertFinalFormat")}</small><strong>{finalFormat}</strong></span><span><small>{t("trimMethod")}</small><strong>{t(mode === "fast" ? "trimFastCut" : "trimPreciseCut")}</strong></span>{mode === "precise" && <><span><small>{t("finalCodec")}</small><strong>{plannedPreciseVideoCodec}</strong></span><span><small>{t("trimPreciseQuality")}</small><strong>{qualityMode === "recommended" ? t("trimQualityRecommended") : effectiveQuality}</strong></span></>}</div><p className={noReencoding ? "trim-plan-copy" : "trim-plan-encode"}>{t(noReencoding ? "trimNoReencodingPlanned" : mode === "precise" ? "trimPreciseReencoding" : "trimReencodingRequired")}</p>{mode === "fast" && <p className="trim-keyframe-warning"><AlertCircle size={13} />{t("trimKeyframeWarning")}</p>}{mode === "fast" && !videoCompatible && <p className="convert-validation"><AlertCircle size={13} />{t("trimFastVideoIncompatible")}</p>}{mode === "fast" && videoCompatible && !audioCompatible && <p className="convert-validation"><AlertCircle size={13} />{t("trimFastAudioIncompatible")}</p>}{mode === "precise" && !preciseFormatSupported && <p className="convert-validation"><AlertCircle size={13} />{t("trimPreciseFormatUnavailable")}</p>}</div></section>

      <section className="compress-panel video-destination-panel"><div><FolderOpen size={19} /><span><strong>{t("destinationFolder")}</strong><small title={destination ?? ""}>{destination ?? t("videoAutomaticDestination")}</small></span></div><button className="secondary-button" type="button" onClick={async () => { const selected = await open({ directory: true, multiple: false }); if (typeof selected === "string") setDestination(selected); }}>{t("chooseFolder")}</button></section><label className="compress-panel trim-output-name"><span>{t("trimOutputName")}</span><input value={outputName} onChange={(event) => setOutputName(event.target.value)} /></label>
      {runError && <p className="video-row-error trim-run-error"><AlertCircle size={14} />{t(runError)}</p>}
      {progress && <section className="video-progress-panel trim-progress"><div><span>{t(mode === "precise" ? "trimPreciseProcessing" : "trimFastCutProcessing")}</span><strong>{progress.name}</strong><b>{progress.percent.toFixed(1)}%</b></div><progress max="100" value={progress.percent} /><small>{formatTrimTime(progress.processedSeconds)} / {formatTrimTime(progress.durationSeconds)}</small></section>}
      {result && !("method" in result) && <section className="compress-panel trim-result"><div className="convert-result-mode"><CheckCircle2 size={14} />{t("trimFastCutCompleted")}</div><div className="convert-result-grid"><span><small>{t("trimRequestedStart")}</small><strong>{formatTrimTime(result.requestedStartSeconds)}</strong></span><span><small>{t("trimRequestedEnd")}</small><strong>{formatTrimTime(result.requestedEndSeconds)}</strong></span><span><small>{t("trimRequestedDuration")}</small><strong>{formatTrimTime(result.requestedDurationSeconds)}</strong></span><span><small>{t("trimDetectedStart")}</small><strong>{formatTrimTime(result.detectedActualStartSeconds)}</strong></span><span><small>{t("trimActualDuration")}</small><strong>{formatTrimTime(result.finalDurationSeconds)}</strong></span><span><small>{t("trimKeyframeOffset")}</small><strong>{result.keyframeOffsetSeconds >= 0 ? "+" : ""}{result.keyframeOffsetSeconds.toFixed(3)} s</strong></span><span><small>{t("convertFinalFormat")}</small><strong>{result.finalFormat}</strong></span><span><small>{t("finalCodec")}</small><strong>{result.finalVideoCodec}</strong></span><span><small>{t("audioCodec")}</small><strong>{result.finalAudioCodec ?? t("noAudio")}</strong></span><span><small>{t("originalSize")}</small><strong>{formatBytes(result.originalSize)}</strong></span><span><small>{t("convertFinalSize")}</small><strong>{formatBytes(result.finalSize)}</strong></span><span><small>{t("processingDuration")}</small><strong>{formatTrimTime(result.processingDurationSeconds)}</strong></span></div><ResultActions result={result} t={t} onPreview={() => setPreviewResult(true)} /></section>}
      {result && "method" in result && <section className="compress-panel trim-result"><div className="convert-result-mode"><CheckCircle2 size={14} />{t("trimPreciseCompleted")}</div><div className="convert-result-grid"><span><small>{t("trimMethod")}</small><strong>{result.method}</strong></span><span><small>{t("trimRequestedStart")}</small><strong>{formatTrimTime(result.requestedStartSeconds)}</strong></span><span><small>{t("trimRequestedEnd")}</small><strong>{formatTrimTime(result.requestedEndSeconds)}</strong></span><span><small>{t("trimRequestedDuration")}</small><strong>{formatTrimTime(result.requestedDurationSeconds)}</strong></span><span><small>{t("trimActualDuration")}</small><strong>{formatTrimTime(result.finalDurationSeconds)}</strong></span><span><small>{t("trimDurationDifference")}</small><strong>{result.durationDifferenceSeconds >= 0 ? "+" : ""}{result.durationDifferenceSeconds.toFixed(3)} s</strong></span><span><small>{t("convertFinalFormat")}</small><strong>{result.finalFormat}</strong></span><span><small>{t("finalCodec")}</small><strong>{result.finalVideoCodec}</strong></span><span><small>{t("audioCodec")}</small><strong>{result.finalAudioCodec ?? t("noAudio")}</strong></span><span><small>{t("trimPreciseQuality")}</small><strong>{result.quality}</strong></span><span><small>{t("resolution")}</small><strong>{result.width} × {result.height}</strong></span><span><small>{t("convertVideoFrameRate")}</small><strong>{result.framesPerSecond.toFixed(3)} fps</strong></span><span><small>{t("originalSize")}</small><strong>{formatBytes(result.originalSize)}</strong></span><span><small>{t("convertFinalSize")}</small><strong>{formatBytes(result.finalSize)}</strong></span><span><small>{t("processingDuration")}</small><strong>{formatTrimTime(result.processingDurationSeconds)}</strong></span></div><ResultActions result={result} t={t} onPreview={() => setPreviewResult(true)} /></section>}
      {isProcessing ? <button className="run-button video-cancel-button trim-video-button" type="button" onClick={() => void cancelActiveCut()}><Ban size={19} />{t(mode === "precise" ? "trimCancelPrecise" : "trimCancel")}</button> : <button className="run-button trim-video-button" type="button" disabled={mode === "fast" ? !canAttemptFastCut : !canAttemptPreciseCut} title={!selectionValid && selectionError ? t(selectionError) : mode === "fast" && !fastCompatible ? t("trimFastIncompatible") : mode === "precise" && !preciseFormatSupported ? t("trimPreciseFormatUnavailable") : undefined} onClick={() => void (mode === "fast" ? runFastCut() : runPreciseCut())}><Scissors size={19} />{t("trimVideoAction")}</button>}
      {previewResult && result && <div className="video-preview-backdrop" role="presentation" onMouseDown={(event) => { if (event.currentTarget === event.target) setPreviewResult(false); }}><section className="video-preview-modal" role="dialog" aria-modal="true" aria-label={t("previewVideo")}><header><div><strong>{result.outputName}</strong><small>{result.finalVideoCodec} · {formatTrimTime(result.finalDurationSeconds)}</small></div><button className="icon-button" type="button" onClick={() => setPreviewResult(false)}><X size={17} /></button></header><video src={convertFileSrc(result.outputPath)} controls autoPlay /></section></div>}
    </>}
  </div>;
}
