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
  CircleGauge,
  Clapperboard,
  FileVideo2,
  FolderOpen,
  HardDrive,
  ImageOff,
  LoaderCircle,
  Play,
  Settings2,
  Trash2,
  UploadCloud,
  Video,
  VolumeX,
  X,
} from "lucide-react";
import { useI18n, type TranslationKey } from "../i18n";
import {
  calculateTargetBitrates,
  estimatedReductionPercent,
  targetSizeToBytes,
  validateTargetSize,
  type TargetSizeUnit,
  type TargetSizeValidationCode,
} from "../utils/videoTargetSize";

type VideoMode = "recommended" | "targetQuality" | "targetFileSize";
type VideoStatus = "analyzing" | "ready" | "processing" | "completed" | "cancelled" | "failed";
type OutputFormat = "mp4" | "webm";

interface VideoInfo {
  path: string;
  name: string;
  format: string;
  durationSeconds: number;
  width: number;
  height: number;
  size: number;
  videoCodec: string;
  audioCodec: string | null;
  thumbnailData: number[] | null;
}

interface VideoItem {
  path: string;
  name: string;
  status: VideoStatus;
  format?: string;
  durationSeconds?: number;
  width?: number;
  height?: number;
  size?: number;
  videoCodec?: string;
  audioCodec?: string | null;
  thumbnailUrl?: string;
  errorCode?: string;
  errorMessage?: string;
  result?: VideoCompressionResult;
  resultKept?: boolean;
}

interface VideoCompressionResult {
  originalSize: number;
  compressedSize: number;
  savedBytes: number;
  reductionPercent: number;
  durationSeconds: number;
  width: number;
  height: number;
  videoCodec: string;
  audioCodec: string | null;
  outputPath: string;
  outputName: string;
  noSizeReduction: boolean;
  quality: number | null;
  encodingSetting: string;
  estimatedSize: number | null;
  estimateDifference: number | null;
  targetSize: number | null;
  initialVideoBitrateKbps: number | null;
  correctedVideoBitrateKbps: number | null;
  attempts: number;
  targetExceeded: boolean;
  pendingConfirmation: boolean;
}

interface VideoCompressionProgress {
  path: string;
  name: string;
  processedSeconds: number;
  durationSeconds: number;
  percent: number;
  phase: "encoding" | "encodingAttempt1" | "adjustingBitrate";
  attempt: number;
}

const videoErrorTranslations: Record<string, TranslationKey> = {
  unsupported_video_format: "videoUnsupportedFormat",
  unsupported_video_file: "videoUnsupportedFile",
  invalid_video: "videoInvalidFile",
  video_not_found: "videoNotFound",
  video_read_error: "videoReadError",
  ffprobe_unavailable: "ffprobeUnavailable",
  ffmpeg_unavailable: "ffmpegUnavailable",
  inspection_failed: "videoInspectionFailed",
  compression_busy: "videoCompressionBusy",
  compression_cancelled: "videoCompressionCancelled",
  cancel_failed: "videoCancelFailed",
  unsupported_codec: "videoUnsupportedCodec",
  destination_inaccessible: "videoDestinationInaccessible",
  disk_full: "videoDiskFull",
  encoding_failed: "videoEncodingFailed",
  output_empty: "videoOutputEmpty",
  output_invalid: "videoOutputInvalid",
  invalid_output_format: "videoInvalidOutputFormat",
  mode_not_implemented: "videoModeNotImplemented",
  delete_failed: "videoDeleteFailed",
  invalid_target_size: "targetSizeNonNumeric",
  target_bitrate_too_low: "targetSizeTooSmall",
  target_webm_not_implemented: "targetWebmNotImplemented",
  automatic_resolution_not_implemented: "automaticResolutionNotImplemented",
};

function fileNameFromPath(path: string) {
  return path.split(/[\\/]/).pop() || path;
}

function formatBytes(bytes: number) {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const unit = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** unit;
  return `${value >= 10 || unit === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
}

function formatDuration(seconds: number) {
  if (!Number.isFinite(seconds) || seconds < 0) return "—";
  const totalTenths = Math.round(seconds * 10);
  const hours = Math.floor(totalTenths / 36_000);
  const minutes = Math.floor((totalTenths % 36_000) / 600);
  const remainingSeconds = ((totalTenths % 600) / 10).toFixed(1).padStart(4, "0");
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(remainingSeconds).padStart(2, "0")}`
    : `${minutes}:${remainingSeconds}`;
}

function estimateVideoSize(size: number, quality: number, outputFormat: OutputFormat, removeAudio: boolean) {
  const points = [[1, 0.18], [30, 0.3], [50, 0.45], [70, 0.62], [85, 0.78], [100, 0.95]] as const;
  let ratio = points[points.length - 1][1];
  for (let index = 1; index < points.length; index += 1) {
    const [rightQuality, rightRatio] = points[index];
    if (quality <= rightQuality) {
      const [leftQuality, leftRatio] = points[index - 1];
      const progress = (quality - leftQuality) / (rightQuality - leftQuality);
      ratio = leftRatio + progress * (rightRatio - leftRatio);
      break;
    }
  }
  const formatFactor = outputFormat === "webm" ? 0.88 : 1;
  return Math.max(1, Math.round(size * ratio * formatFactor * (removeAudio ? 0.92 : 1)));
}

function formatBitrate(kbps: number) {
  if (!Number.isFinite(kbps) || kbps < 0) return "—";
  if (kbps === 0) return "0 kbps";
  return kbps >= 1000 ? `${(kbps / 1000).toFixed(2)} Mbps` : `${Math.round(kbps)} kbps`;
}

function parseCommandError(error: unknown): { code: string; message?: string } {
  if (typeof error === "object" && error !== null) {
    const value = error as Record<string, unknown>;
    return {
      code: String(value.code ?? "inspection_failed"),
      message: typeof value.message === "string" ? value.message : undefined,
    };
  }
  try {
    const parsed = JSON.parse(String(error)) as { code?: string; message?: string };
    return { code: parsed.code ?? "inspection_failed", message: parsed.message };
  } catch {
    return { code: "inspection_failed", message: String(error) };
  }
}

function thumbnailUrl(data: number[] | null) {
  if (!data?.length) return undefined;
  return URL.createObjectURL(new Blob([new Uint8Array(data)], { type: "image/png" }));
}

export function CompressVideoPage() {
  const { t, language, setLanguage } = useI18n();
  const [items, setItems] = useState<VideoItem[]>([]);
  const [destination, setDestination] = useState<string | null>(null);
  const [mode, setMode] = useState<VideoMode>("recommended");
  const [quality, setQuality] = useState(70);
  const [targetSizeInput, setTargetSizeInput] = useState("25");
  const [targetSizeUnit, setTargetSizeUnit] = useState<TargetSizeUnit>("MB");
  const [allowAutomaticResolutionReduction, setAllowAutomaticResolutionReduction] = useState(false);
  const [nonReducingAcknowledgement, setNonReducingAcknowledgement] = useState<string | null>(null);
  const [preserveResolution, setPreserveResolution] = useState(true);
  const [removeAudio, setRemoveAudio] = useState(false);
  const [preserveMetadata, setPreserveMetadata] = useState(true);
  const [outputFormat, setOutputFormat] = useState<OutputFormat>("mp4");
  const [isProcessing, setIsProcessing] = useState(false);
  const [activeProgress, setActiveProgress] = useState<VideoCompressionProgress | null>(null);
  const [batchPosition, setBatchPosition] = useState({ current: 0, total: 0 });
  const [previewResult, setPreviewResult] = useState<VideoCompressionResult | null>(null);
  const [pageError, setPageError] = useState<string | null>(null);
  const thumbnailUrls = useRef(new Set<string>());
  const cancellationRequested = useRef(false);

  const addPaths = useCallback(async (paths: string[]) => {
    const unique = [...new Set(paths)].filter((path) => !items.some((item) => item.path === path));
    if (!unique.length) return;

    setItems((current) => [
      ...current,
      ...unique.map((path) => ({
        path,
        name: fileNameFromPath(path),
        status: "analyzing" as const,
      })),
    ]);

    for (const path of unique) {
      try {
        const info = await invoke<VideoInfo>("inspect_video", { path });
        const preview = thumbnailUrl(info.thumbnailData);
        if (preview) thumbnailUrls.current.add(preview);
        setItems((current) => current.map((item) => item.path === path ? {
          path: info.path,
          name: info.name,
          format: info.format,
          durationSeconds: info.durationSeconds,
          width: info.width,
          height: info.height,
          size: info.size,
          videoCodec: info.videoCodec,
          audioCodec: info.audioCodec,
          thumbnailUrl: preview,
          status: "ready",
        } : item));
      } catch (error) {
        const parsed = parseCommandError(error);
        setItems((current) => current.map((item) => item.path === path ? {
          ...item,
          status: "failed",
          errorCode: parsed.code,
          errorMessage: parsed.message,
        } : item));
      }
    }
  }, [items]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "drop") void addPaths(event.payload.paths);
    }).then((cleanup) => {
      if (disposed) cleanup(); else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [addPaths]);

  useEffect(() => () => {
    thumbnailUrls.current.forEach((url) => URL.revokeObjectURL(url));
    thumbnailUrls.current.clear();
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<VideoCompressionProgress>("video-compression-progress", (event) => {
      if (!disposed) setActiveProgress(event.payload);
    }).then((cleanup) => {
      if (disposed) cleanup(); else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  async function chooseVideos() {
    if (isProcessing) return;
    const selected = await open({
      multiple: true,
      directory: false,
      filters: [{ name: "MP4, MOV, MKV, AVI, WebM, M4V", extensions: ["mp4", "mov", "mkv", "avi", "webm", "m4v"] }],
    });
    if (selected) await addPaths(Array.isArray(selected) ? selected : [selected]);
  }

  async function chooseDestination() {
    if (isProcessing) return;
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === "string") setDestination(selected);
  }

  async function removeItem(path: string) {
    if (isProcessing) return;
    const pending = items.find((item) => item.path === path)?.result;
    if (pending?.pendingConfirmation) {
      await invoke("delete_video_result", { path: pending.outputPath }).catch(() => undefined);
    }
    setItems((current) => {
      const removed = current.find((item) => item.path === path);
      if (removed?.thumbnailUrl) {
        URL.revokeObjectURL(removed.thumbnailUrl);
        thumbnailUrls.current.delete(removed.thumbnailUrl);
      }
      return current.filter((item) => item.path !== path);
    });
  }

  async function clearAll() {
    if (isProcessing) return;
    await Promise.all(items.filter((item) => item.result?.pendingConfirmation).map((item) =>
      invoke("delete_video_result", { path: item.result!.outputPath }).catch(() => undefined),
    ));
    items.forEach((item) => {
      if (item.thumbnailUrl) URL.revokeObjectURL(item.thumbnailUrl);
    });
    thumbnailUrls.current.clear();
    setItems([]);
  }

  const readyItems = items.filter((item) => item.status === "ready");
  const totalInputSize = readyItems.reduce((total, item) => total + (item.size ?? 0), 0);
  const parsedTargetSize = Number(targetSizeInput);
  const targetBytesPerVideo = Number.isFinite(parsedTargetSize) && parsedTargetSize > 0
    ? targetSizeToBytes(parsedTargetSize, targetSizeUnit)
    : 0;
  const targetBatchSize = targetBytesPerVideo * readyItems.length;
  const outputAudioKbps = removeAudio ? 0 : outputFormat === "mp4" ? 160 : 128;
  const targetInputs = readyItems.map((item) => ({
    durationSeconds: item.durationSeconds ?? 0,
    audioKbps: item.audioCodec ? outputAudioKbps : 0,
    width: item.width ?? 0,
    height: item.height ?? 0,
  }));
  const targetValidation = validateTargetSize(targetSizeInput, targetSizeUnit, targetInputs);
  const bitrateEstimates = targetInputs
    .map((input) => calculateTargetBitrates(targetBytesPerVideo, input.durationSeconds, input.audioKbps))
    .filter((estimate): estimate is NonNullable<typeof estimate> => estimate !== null);
  const averageBitrate = bitrateEstimates.length ? bitrateEstimates.reduce((total, estimate) => ({
    totalKbps: total.totalKbps + estimate.totalKbps / bitrateEstimates.length,
    videoKbps: total.videoKbps + estimate.videoKbps / bitrateEstimates.length,
    audioKbps: total.audioKbps + estimate.audioKbps / bitrateEstimates.length,
    overheadKbps: total.overheadKbps + estimate.overheadKbps / bitrateEstimates.length,
  }), { totalKbps: 0, videoKbps: 0, audioKbps: 0, overheadKbps: 0 }) : null;
  const displayedBitrate = targetValidation === null || targetValidation === "tooSmall" ? averageBitrate : null;
  const estimatedTargetReduction = estimatedReductionPercent(totalInputSize, targetBatchSize);
  const nonReducingTarget = targetValidation === null && targetBytesPerVideo > 0
    && readyItems.some((item) => targetBytesPerVideo >= (item.size ?? 0));
  const acknowledgementKey = `${targetSizeInput}:${targetSizeUnit}:${readyItems.map((item) => `${item.path}:${item.size}`).join("|")}`;
  const nonReducingAcknowledged = nonReducingAcknowledgement === acknowledgementKey;
  const estimatedOutputSize = useMemo(() => {
    if (!readyItems.length) return 0;
    if (mode === "targetFileSize") return targetValidation === null ? targetBatchSize : 0;
    if (mode === "targetQuality") {
      return readyItems.reduce((total, item) => total + estimateVideoSize(item.size ?? 0, quality, outputFormat, removeAudio), 0);
    }
    return Math.round(totalInputSize * 0.58 * (removeAudio ? 0.92 : 1));
  }, [mode, quality, readyItems, removeAudio, outputFormat, targetBatchSize, targetValidation, totalInputSize]);

  const targetValidationTranslations: Record<TargetSizeValidationCode, TranslationKey> = {
    nonNumeric: "targetSizeNonNumeric",
    zero: "targetSizeZero",
    negative: "targetSizeNegative",
    tooLarge: "targetSizeTooLarge",
    tooSmall: "targetSizeTooSmall",
  };
  const targetCompressionBlocked = mode === "targetFileSize" && (
    targetValidation !== null
    || outputFormat !== "mp4"
    || allowAutomaticResolutionReduction
    || (nonReducingTarget && !nonReducingAcknowledged)
  );

  const qualityDescription = quality <= 30
    ? t("videoQualitySmaller")
    : quality <= 60
      ? t("videoQualityBalanced")
      : quality <= 85
        ? t("videoQualityHigh")
        : t("videoQualityMaximum");

  function translatedError(item: VideoItem) {
    const key = item.errorCode ? videoErrorTranslations[item.errorCode] : undefined;
    return key ? t(key) : item.errorMessage || t("videoInspectionFailed");
  }

  async function runCompression() {
    if (mode === "targetFileSize") {
      if (outputFormat !== "mp4") { setPageError(t("targetWebmNotImplemented")); return; }
      if (targetValidation) { setPageError(t(targetValidationTranslations[targetValidation])); return; }
      if (allowAutomaticResolutionReduction) { setPageError(t("automaticResolutionNotImplemented")); return; }
      if (nonReducingTarget && !nonReducingAcknowledged) { setPageError(t("targetNotSmallerThanOriginal")); return; }
    }
    const queue = items.filter((item) => item.status === "ready");
    if (!queue.length || isProcessing) return;

    setIsProcessing(true);
    setPageError(null);
    setBatchPosition({ current: 1, total: queue.length });
    cancellationRequested.current = false;

    for (let index = 0; index < queue.length; index += 1) {
      const item = queue[index];
      if (cancellationRequested.current) break;
      setBatchPosition({ current: index + 1, total: queue.length });
      setActiveProgress({ path: item.path, name: item.name, processedSeconds: 0, durationSeconds: item.durationSeconds ?? 0, percent: 0, phase: mode === "targetFileSize" ? "encodingAttempt1" : "encoding", attempt: 1 });
      setItems((current) => current.map((entry) => entry.path === item.path ? {
        ...entry,
        status: "processing",
        errorCode: undefined,
        errorMessage: undefined,
        result: undefined,
        resultKept: false,
      } : entry));

      try {
        const result = await invoke<VideoCompressionResult>("compress_recommended_video", {
          request: {
            path: item.path,
            destinationDir: destination,
            mode,
            preserveResolution,
            removeAudio,
            preserveMetadata,
            outputFormat,
            quality: mode === "targetQuality" ? quality : null,
            estimatedSize: mode === "targetQuality" ? estimateVideoSize(item.size ?? 0, quality, outputFormat, removeAudio) : mode === "targetFileSize" ? targetBytesPerVideo : null,
            targetSizeBytes: mode === "targetFileSize" ? targetBytesPerVideo : null,
            allowAutomaticResolutionReduction: mode === "targetFileSize" ? allowAutomaticResolutionReduction : false,
          },
        });
        setItems((current) => current.map((entry) => entry.path === item.path ? {
          ...entry,
          status: "completed",
          result,
        } : entry));
      } catch (error) {
        const parsed = parseCommandError(error);
        const cancelled = parsed.code === "compression_cancelled";
        setItems((current) => current.map((entry) => entry.path === item.path ? {
          ...entry,
          status: cancelled ? "cancelled" : "failed",
          errorCode: parsed.code,
          errorMessage: parsed.message,
        } : entry));
        if (cancelled || cancellationRequested.current) break;
      }
    }

    setIsProcessing(false);
    setActiveProgress(null);
    cancellationRequested.current = false;
  }

  async function cancelCompression() {
    cancellationRequested.current = true;
    try {
      await invoke<boolean>("cancel_video_compression");
    } catch (error) {
      const parsed = parseCommandError(error);
      setPageError(parsed.message || t(videoErrorTranslations[parsed.code] ?? "videoCancelFailed"));
    }
  }

  async function deleteResult(item: VideoItem) {
    if (!item.result || isProcessing) return;
    try {
      await invoke("delete_video_result", { path: item.result.outputPath });
      if (previewResult?.outputPath === item.result.outputPath) setPreviewResult(null);
      setItems((current) => current.map((entry) => entry.path === item.path ? {
        ...entry,
        status: "ready",
        result: undefined,
        resultKept: false,
      } : entry));
    } catch (error) {
      const parsed = parseCommandError(error);
      setPageError(parsed.message || t(videoErrorTranslations[parsed.code] ?? "videoDeleteFailed"));
    }
  }

  async function keepTargetResult(item: VideoItem) {
    if (!item.result?.pendingConfirmation || isProcessing) return;
    try {
      const kept = await invoke<{ outputPath: string; outputName: string }>("keep_video_target_result", { path: item.result.outputPath });
      setItems((current) => current.map((entry) => entry.path === item.path && entry.result ? {
        ...entry,
        resultKept: true,
        result: { ...entry.result, ...kept, pendingConfirmation: false },
      } : entry));
    } catch (error) {
      const parsed = parseCommandError(error);
      setPageError(parsed.message || t("videoOutputInvalid"));
    }
  }

  return (
    <div className="page video-compress-page">
      <header className="page-header compression-header video-header">
        <div>
          <span className="eyebrow">{t("videoModuleLabel")}</span>
          <h1>{t("videoCompressTitle")}</h1>
          <p>{t("videoCompressSubtitle")}</p>
        </div>
        <div className="compress-header-actions">
          <label className="language-select" title={t("language")}>
            <span>{language.toUpperCase()}</span>
            <select value={language} onChange={(event) => setLanguage(event.target.value as "en" | "fr" | "ar")}>
              <option value="en">English</option>
              <option value="fr">Français</option>
              <option value="ar">العربية</option>
            </select>
          </label>
          <div className="header-status"><Clapperboard size={17} /><span>FFprobe</span></div>
        </div>
      </header>

      <section className={`video-drop-zone ${isProcessing ? "disabled" : ""}`} onClick={() => void chooseVideos()}>
        <div className="video-drop-icon"><UploadCloud size={34} /></div>
        <div>
          <h2>{t("videoDropTitle")}</h2>
          <p>{t("videoDropSubtitle")}</p>
        </div>
        <button className="primary-button" type="button" disabled={isProcessing} onClick={(event) => { event.stopPropagation(); void chooseVideos(); }}>
          <FileVideo2 size={17} />{t("addVideos")}
        </button>
      </section>

      <section className="compress-panel video-files-panel">
        <div className="compress-panel-header">
          <div><h2>{t("selectedVideos")}</h2><span>{t("videoCount", { count: items.length })}</span></div>
          {!!items.length && <button className="text-button danger" type="button" disabled={isProcessing} onClick={clearAll}><Trash2 size={14} />{t("clearAll")}</button>}
        </div>

        {!items.length ? (
          <div className="video-empty-state"><Video size={30} /><strong>{t("noVideos")}</strong><p>{t("noVideosHint")}</p></div>
        ) : (
          <div className="video-file-list">
            {items.map((item) => (
              <article className={`video-file-row ${item.status}`} key={item.path}>
                <div className="video-thumbnail">
                  {item.thumbnailUrl ? <img src={item.thumbnailUrl} alt="" /> : item.status === "analyzing" ? <LoaderCircle className="spin" size={22} /> : <ImageOff size={21} />}
                  {item.durationSeconds !== undefined && <span>{formatDuration(item.durationSeconds)}</span>}
                </div>

                <div className="video-file-details">
                  <div className="video-file-title">
                    <div><strong title={item.path}>{item.name}</strong><small title={item.path}>{item.path}</small></div>
                    <div className={`video-status ${item.status}`}>
                      {item.status === "analyzing" || item.status === "processing" ? <LoaderCircle className="spin" size={13} /> : item.status === "ready" || item.status === "completed" ? <CheckCircle2 size={13} /> : item.status === "cancelled" ? <Ban size={13} /> : <AlertCircle size={13} />}
                      <span>{t(item.status === "analyzing" ? "analyzingVideo" : item.status === "processing" ? "videoCompressing" : item.status === "ready" ? "videoReady" : item.status === "completed" ? "videoCompleted" : item.status === "cancelled" ? "videoCancelled" : "videoFailed")}</span>
                    </div>
                  </div>

                  {item.status === "failed" || item.status === "cancelled" ? (
                    <div className="video-row-error"><AlertCircle size={14} /><span>{translatedError(item)}</span></div>
                  ) : (
                    <div className="video-metadata-grid">
                      <span><small>{t("format")}</small><strong>{item.format ?? "—"}</strong></span>
                      <span><small>{t("videoDuration")}</small><strong className="technical-value">{item.durationSeconds === undefined ? "—" : formatDuration(item.durationSeconds)}</strong></span>
                      <span><small>{t("resolution")}</small><strong className="technical-value">{item.width ? `${item.width} × ${item.height}` : "—"}</strong></span>
                      <span><small>{t("fileSize")}</small><strong className="technical-value">{item.size ? formatBytes(item.size) : "—"}</strong></span>
                      <span><small>{t("videoCodec")}</small><strong className="technical-value">{item.videoCodec ?? "—"}</strong></span>
                      <span><small>{t("audioCodec")}</small><strong className="technical-value">{item.audioCodec ?? (item.status === "ready" ? t("noAudio") : "—")}</strong></span>
                    </div>
                  )}
                  {item.result && <div className={`video-result-strip ${item.result.noSizeReduction || item.result.targetExceeded ? "no-reduction" : ""}`}>
                    {item.result.targetExceeded && <div className="video-no-reduction"><AlertCircle size={15} /><strong>{t("targetExceeded")}</strong><span>{t("targetExceededHint")}</span></div>}
                    {item.result.noSizeReduction && !item.result.pendingConfirmation && <div className="video-no-reduction"><AlertCircle size={15} /><strong>{t("noSizeReduction")}</strong><span>{t("noSizeReductionHint")}</span></div>}
                    <div className="video-result-metrics">
                      <span><small>{t("originalSize")}</small><strong>{formatBytes(item.result.originalSize)}</strong></span>
                      <span><small>{t("compressedSize")}</small><strong>{formatBytes(item.result.compressedSize)}</strong></span>
                      <span><small>{t("spaceSaved")}</small><strong>{formatBytes(item.result.savedBytes)}</strong></span>
                      <span><small>{t("reduction")}</small><strong>{item.result.reductionPercent.toFixed(1)}%</strong></span>
                      <span><small>{t("videoDuration")}</small><strong>{formatDuration(item.result.durationSeconds)}</strong></span>
                      <span><small>{t("finalResolution")}</small><strong>{item.result.width} × {item.result.height}</strong></span>
                      <span><small>{t("finalCodec")}</small><strong>{item.result.videoCodec}{item.result.audioCodec ? ` / ${item.result.audioCodec}` : ""}</strong></span>
                      {item.result.quality !== null && <span><small>{t("selectedQuality")}</small><strong>{item.result.quality}%</strong></span>}
                      <span title={item.result.encodingSetting}><small>{t("encodingSetting")}</small><strong>{item.result.encodingSetting}</strong></span>
                      {item.result.estimatedSize !== null && <span><small>{t("estimatedSizeResult")}</small><strong>{formatBytes(item.result.estimatedSize)}</strong></span>}
                      {item.result.targetSize !== null && <span><small>{t("targetSizeResult")}</small><strong>{formatBytes(item.result.targetSize)}</strong></span>}
                      {item.result.initialVideoBitrateKbps !== null && <span><small>{t("initialVideoBitrate")}</small><strong>{formatBitrate(item.result.initialVideoBitrateKbps)}</strong></span>}
                      {item.result.correctedVideoBitrateKbps !== null && <span><small>{t("correctedVideoBitrate")}</small><strong>{formatBitrate(item.result.correctedVideoBitrateKbps)}</strong></span>}
                      {item.result.targetSize !== null && <span><small>{t("encodingAttempts")}</small><strong>{item.result.attempts}</strong></span>}
                      {item.result.estimateDifference !== null && <span><small>{t("estimateDifference")}</small><strong>{item.result.estimateDifference >= 0 ? "+" : "−"}{formatBytes(Math.abs(item.result.estimateDifference))}</strong></span>}
                    </div>
                    <div className="video-result-location"><FolderOpen size={13} /><span title={item.result.outputPath}>{item.result.outputPath}</span></div>
                    <div className="video-result-actions">
                      <button className="text-button" type="button" onClick={() => setPreviewResult(item.result!)}><Play size={13} />{t("previewVideo")}</button>
                      <button className="text-button" type="button" onClick={() => void openPath(item.result!.outputPath)}><Video size={13} />{t("openVideo")}</button>
                      <button className="text-button" type="button" onClick={() => void revealItemInDir(item.result!.outputPath)}><FolderOpen size={13} />{t("openFolder")}</button>
                      {item.result.pendingConfirmation && <><button className="text-button danger" type="button" onClick={() => void deleteResult(item)}><Trash2 size={13} />{t("deleteResult")}</button><button className="text-button" type="button" onClick={() => void keepTargetResult(item)}><Check size={13} />{t("keepResult")}</button></>}
                      {item.result.noSizeReduction && !item.result.pendingConfirmation && !item.resultKept && <><button className="text-button danger" type="button" onClick={() => void deleteResult(item)}><Trash2 size={13} />{t("deleteResult")}</button><button className="text-button" type="button" onClick={() => setItems((current) => current.map((entry) => entry.path === item.path ? { ...entry, resultKept: true } : entry))}><Check size={13} />{t("keepResult")}</button></>}
                      {item.resultKept && <span className="video-result-kept"><Check size={13} />{t("resultKept")}</span>}
                    </div>
                  </div>}
                </div>

                <button className="video-remove-button" type="button" disabled={isProcessing} onClick={() => void removeItem(item.path)} title={t("remove")}>
                  <Trash2 size={15} /><span>{t("remove")}</span>
                </button>
              </article>
            ))}
          </div>
        )}
      </section>

      <div className="video-settings-grid">
        <section className="compress-panel video-mode-panel">
          <div className="video-section-title"><CircleGauge size={18} /><div><h2>{t("videoCompressionMode")}</h2><p>{t("videoCompressionModeHint")}</p></div></div>
          <div className="video-mode-options">
            {(["recommended", "targetQuality", "targetFileSize"] as VideoMode[]).map((value) => (
              <button type="button" disabled={isProcessing} className={mode === value ? "active" : ""} key={value} onClick={() => { setMode(value); if (value === "targetFileSize") { setOutputFormat("mp4"); setPreserveResolution(true); } }}>
                <span className="radio-dot" /><span><strong>{t(`video${value[0].toUpperCase()}${value.slice(1)}` as TranslationKey)}</strong><small>{t(`video${value[0].toUpperCase()}${value.slice(1)}Hint` as TranslationKey)}</small></span>
              </button>
            ))}
          </div>
          {mode === "targetQuality" && <div className="video-quality-controls">
            <label className="video-range-control"><span><strong>{t("videoQuality")}</strong><b>{quality}%</b></span><input type="range" min="1" max="100" value={quality} onChange={(event) => setQuality(Number(event.target.value))} /><small>{qualityDescription}</small></label>
            <div className="video-quality-presets">
              {([["videoPresetMaximumCompression", 20], ["videoPresetBalanced", 50], ["videoPresetHighQuality", 75], ["videoPresetNearOriginal", 95]] as [TranslationKey, number][]).map(([label, value]) => <button type="button" disabled={isProcessing} className={quality === value ? "active" : ""} key={label} onClick={() => setQuality(value)}>{t(label)}</button>)}
            </div>
            <p className="video-quality-warning"><AlertCircle size={13} />{t("videoQualityEstimateWarning")}</p>
          </div>}
          {mode === "targetFileSize" && <div className="video-target-size-controls">
            <label className="video-target-size"><span>{t("targetFileSizePerVideo")}</span><div><input type="text" inputMode="decimal" value={targetSizeInput} onChange={(event) => setTargetSizeInput(event.target.value)} aria-invalid={targetValidation !== null} /><select value={targetSizeUnit} onChange={(event) => setTargetSizeUnit(event.target.value as TargetSizeUnit)} aria-label={t("targetSizeUnit")}>{(["KB", "MB", "GB"] as TargetSizeUnit[]).map((unit) => <option value={unit} key={unit}>{unit}</option>)}</select></div></label>
            {targetValidation && <p className="video-target-validation"><AlertCircle size={13} />{t(targetValidationTranslations[targetValidation])}</p>}
            <div className="video-target-presets" aria-label={t("targetSizePresets")}>{[5, 10, 25, 50, 100].map((value) => <button type="button" disabled={isProcessing} className={targetSizeUnit === "MB" && parsedTargetSize === value ? "active" : ""} key={value} onClick={() => { setTargetSizeInput(String(value)); setTargetSizeUnit("MB"); }}>{value} MB</button>)}</div>
            <div className="video-target-estimates">
              <span><small>{t("originalSize")}</small><strong>{totalInputSize ? formatBytes(totalInputSize) : "—"}</strong></span>
              <span><small>{t("estimatedReduction")}</small><strong>{targetValidation === null && totalInputSize ? `${estimatedTargetReduction.toFixed(1)}%` : "—"}</strong></span>
              <span><small>{t("estimatedTotalBitrate")}</small><strong>{displayedBitrate ? formatBitrate(displayedBitrate.totalKbps) : "—"}</strong></span>
              <span><small>{t("estimatedVideoBitrate")}</small><strong>{displayedBitrate ? formatBitrate(displayedBitrate.videoKbps) : "—"}</strong></span>
              <span><small>{t("estimatedAudioBitrate")}</small><strong>{displayedBitrate ? formatBitrate(displayedBitrate.audioKbps) : "—"}</strong></span>
            </div>
            {nonReducingTarget && !nonReducingAcknowledged && <div className="video-target-confirmation"><p>{t("targetNotSmallerThanOriginal")}</p><div><button type="button" onClick={() => { setNonReducingAcknowledgement(null); setMode("recommended"); }}>{t("cancelTargetSize")}</button><button className="continue" type="button" onClick={() => setNonReducingAcknowledgement(acknowledgementKey)}>{t("continueTargetSize")}</button></div></div>}
            {nonReducingTarget && nonReducingAcknowledged && <p className="video-target-acknowledged"><Check size={13} />{t("targetNotSmallerAcknowledged")}</p>}
            <label className="video-auto-resolution"><input type="checkbox" disabled={isProcessing} checked={allowAutomaticResolutionReduction} onChange={(event) => setAllowAutomaticResolutionReduction(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><span><strong>{t("allowAutomaticResolutionReduction")}</strong><small>{t("allowAutomaticResolutionReductionHint")}</small></span></label>
            {targetValidation === "tooSmall" && <p className="video-low-quality-warning"><AlertCircle size={13} />{t("targetLowQualityWarning")}</p>}
            <p className="video-quality-warning"><AlertCircle size={13} />{t("targetSizeVariationWarning")}</p>
          </div>}
        </section>

        <section className="compress-panel video-options-panel">
          <div className="video-section-title"><Settings2 size={18} /><div><h2>{t("videoOptions")}</h2><p>{t("videoOptionsHint")}</p></div></div>
          <div className="video-check-options">
            <label><input type="checkbox" disabled={isProcessing || mode === "targetFileSize"} checked={preserveResolution} onChange={(event) => setPreserveResolution(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><span><strong>{t("preserveResolution")}</strong><small>{t("preserveResolutionHint")}</small></span></label>
            <label><input type="checkbox" disabled={isProcessing} checked={removeAudio} onChange={(event) => setRemoveAudio(event.target.checked)} /><span className="custom-check"><VolumeX size={13} /></span><span><strong>{t("removeAudio")}</strong><small>{t("removeAudioHint")}</small></span></label>
            <label><input type="checkbox" disabled={isProcessing} checked={preserveMetadata} onChange={(event) => setPreserveMetadata(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><span><strong>{t("preserveMetadata")}</strong><small>{t("preserveMetadataHint")}</small></span></label>
          </div>
          <div className="video-output-format">
            <strong>{t("outputFormat")}</strong>
            <div>{(["mp4", "webm"] as OutputFormat[]).map((value) => <button type="button" disabled={isProcessing || (mode === "targetFileSize" && value === "webm")} className={outputFormat === value ? "active" : ""} onClick={() => setOutputFormat(value)} key={value}>{value === "mp4" ? "MP4" : "WebM"}</button>)}</div>
          </div>
        </section>
      </div>

      <section className="compress-panel video-destination-panel">
        <div className="video-section-title"><HardDrive size={18} /><div><h2>{t("destination")}</h2><p className="technical-path">{destination ?? t("videoAutomaticDestination")}</p></div></div>
        <button className="secondary-button" type="button" disabled={isProcessing} onClick={() => void chooseDestination()}><FolderOpen size={15} />{t("chooseFolder")}</button>
      </section>

      <section className="video-estimate-panel">
        <div><span>{t("estimatedOutputSize")}</span><strong>{estimatedOutputSize ? formatBytes(estimatedOutputSize) : "—"}</strong><small>{t("estimatedOutputSizeHint")}</small></div>
        <div><span>{t("totalInputSize")}</span><strong>{totalInputSize ? formatBytes(totalInputSize) : "—"}</strong><small>{t("videoFilesReady", { count: readyItems.length })}</small></div>
      </section>

      {activeProgress && <section className="video-progress-panel"><div><span>{t(activeProgress.phase === "encodingAttempt1" ? "encodingAttemptOne" : activeProgress.phase === "adjustingBitrate" ? "adjustingBitrate" : "videoProcessingFile", { current: batchPosition.current, total: batchPosition.total })}</span><strong>{activeProgress.name}</strong><b>{activeProgress.percent.toFixed(1)}%</b></div><progress max="100" value={activeProgress.percent} /><small>{formatDuration(activeProgress.processedSeconds)} / {formatDuration(activeProgress.durationSeconds)}</small></section>}
      {pageError && <div className="page-error"><AlertCircle size={16} />{pageError}</div>}
      {isProcessing ? <button className="run-button video-cancel-button" type="button" onClick={() => void cancelCompression()}><Ban size={19} />{t("cancelVideoCompression")}</button> : <button className="run-button video-compress-button" type="button" disabled={targetCompressionBlocked || !readyItems.length} title={targetCompressionBlocked ? t("targetSettingsInvalid") : undefined} onClick={() => void runCompression()}><Clapperboard size={19} />{t("compressVideos")}</button>}
      {mode === "targetFileSize" && allowAutomaticResolutionReduction && <p className="video-disabled-hint">{t("automaticResolutionNotImplemented")}</p>}

      {previewResult && <div className="video-preview-backdrop" role="presentation" onMouseDown={(event) => { if (event.currentTarget === event.target) setPreviewResult(null); }}><section className="video-preview-modal" role="dialog" aria-modal="true" aria-label={t("previewVideo")}><header><div><strong>{previewResult.outputName}</strong><small>{previewResult.width} × {previewResult.height} · {previewResult.videoCodec} · {formatDuration(previewResult.durationSeconds)}</small></div><button className="icon-button" type="button" onClick={() => setPreviewResult(null)} aria-label={t("closePreview")}><X size={17} /></button></header><video src={convertFileSrc(previewResult.outputPath)} controls autoPlay /></section></div>}
    </div>
  );
}
