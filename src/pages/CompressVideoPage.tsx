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
}

interface VideoCompressionProgress {
  path: string;
  name: string;
  processedSeconds: number;
  durationSeconds: number;
  percent: number;
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
  const [quality, setQuality] = useState(75);
  const [targetSizeMb, setTargetSizeMb] = useState(25);
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

  function removeItem(path: string) {
    if (isProcessing) return;
    setItems((current) => {
      const removed = current.find((item) => item.path === path);
      if (removed?.thumbnailUrl) {
        URL.revokeObjectURL(removed.thumbnailUrl);
        thumbnailUrls.current.delete(removed.thumbnailUrl);
      }
      return current.filter((item) => item.path !== path);
    });
  }

  function clearAll() {
    if (isProcessing) return;
    items.forEach((item) => {
      if (item.thumbnailUrl) URL.revokeObjectURL(item.thumbnailUrl);
    });
    thumbnailUrls.current.clear();
    setItems([]);
  }

  const readyItems = items.filter((item) => item.status === "ready");
  const totalInputSize = readyItems.reduce((total, item) => total + (item.size ?? 0), 0);
  const estimatedOutputSize = useMemo(() => {
    if (!readyItems.length) return 0;
    if (mode === "targetFileSize") return targetSizeMb * 1024 * 1024 * readyItems.length;
    const ratio = mode === "targetQuality" ? 0.25 + (quality / 100) * 0.55 : 0.58;
    const audioFactor = removeAudio ? 0.92 : 1;
    return Math.round(totalInputSize * ratio * audioFactor);
  }, [mode, quality, readyItems.length, removeAudio, targetSizeMb, totalInputSize]);

  function translatedError(item: VideoItem) {
    const key = item.errorCode ? videoErrorTranslations[item.errorCode] : undefined;
    return key ? t(key) : item.errorMessage || t("videoInspectionFailed");
  }

  async function runRecommendedCompression() {
    if (mode !== "recommended") {
      setPageError(t("videoModeNotImplemented"));
      return;
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
      setActiveProgress({ path: item.path, name: item.name, processedSeconds: 0, durationSeconds: item.durationSeconds ?? 0, percent: 0 });
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
                  {item.result && <div className={`video-result-strip ${item.result.noSizeReduction ? "no-reduction" : ""}`}>
                    {item.result.noSizeReduction && <div className="video-no-reduction"><AlertCircle size={15} /><strong>{t("noSizeReduction")}</strong><span>{t("noSizeReductionHint")}</span></div>}
                    <div className="video-result-metrics">
                      <span><small>{t("originalSize")}</small><strong>{formatBytes(item.result.originalSize)}</strong></span>
                      <span><small>{t("compressedSize")}</small><strong>{formatBytes(item.result.compressedSize)}</strong></span>
                      <span><small>{t("spaceSaved")}</small><strong>{formatBytes(item.result.savedBytes)}</strong></span>
                      <span><small>{t("reduction")}</small><strong>{item.result.reductionPercent.toFixed(1)}%</strong></span>
                      <span><small>{t("videoDuration")}</small><strong>{formatDuration(item.result.durationSeconds)}</strong></span>
                      <span><small>{t("finalResolution")}</small><strong>{item.result.width} × {item.result.height}</strong></span>
                      <span><small>{t("finalCodec")}</small><strong>{item.result.videoCodec}{item.result.audioCodec ? ` / ${item.result.audioCodec}` : ""}</strong></span>
                    </div>
                    <div className="video-result-location"><FolderOpen size={13} /><span title={item.result.outputPath}>{item.result.outputPath}</span></div>
                    <div className="video-result-actions">
                      <button className="text-button" type="button" onClick={() => setPreviewResult(item.result!)}><Play size={13} />{t("previewVideo")}</button>
                      <button className="text-button" type="button" onClick={() => void openPath(item.result!.outputPath)}><Video size={13} />{t("openVideo")}</button>
                      <button className="text-button" type="button" onClick={() => void revealItemInDir(item.result!.outputPath)}><FolderOpen size={13} />{t("openFolder")}</button>
                      {item.result.noSizeReduction && !item.resultKept && <><button className="text-button danger" type="button" onClick={() => void deleteResult(item)}><Trash2 size={13} />{t("deleteResult")}</button><button className="text-button" type="button" onClick={() => setItems((current) => current.map((entry) => entry.path === item.path ? { ...entry, resultKept: true } : entry))}><Check size={13} />{t("keepResult")}</button></>}
                      {item.resultKept && <span className="video-result-kept"><Check size={13} />{t("resultKept")}</span>}
                    </div>
                  </div>}
                </div>

                <button className="video-remove-button" type="button" disabled={isProcessing} onClick={() => removeItem(item.path)} title={t("remove")}>
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
              <button type="button" disabled={isProcessing} className={mode === value ? "active" : ""} key={value} onClick={() => setMode(value)}>
                <span className="radio-dot" /><span><strong>{t(`video${value[0].toUpperCase()}${value.slice(1)}` as TranslationKey)}</strong><small>{t(`video${value[0].toUpperCase()}${value.slice(1)}Hint` as TranslationKey)}</small></span>
              </button>
            ))}
          </div>
          {mode === "targetQuality" && <label className="video-range-control"><span><strong>{t("videoQuality")}</strong><b>{quality}%</b></span><input type="range" min="10" max="100" value={quality} onChange={(event) => setQuality(Number(event.target.value))} /></label>}
          {mode === "targetFileSize" && <label className="video-target-size"><span>{t("targetFileSizePerVideo")}</span><div><input type="number" min="1" step="1" value={targetSizeMb} onChange={(event) => setTargetSizeMb(Math.max(1, Number(event.target.value) || 1))} /><b>MB</b></div></label>}
        </section>

        <section className="compress-panel video-options-panel">
          <div className="video-section-title"><Settings2 size={18} /><div><h2>{t("videoOptions")}</h2><p>{t("videoOptionsHint")}</p></div></div>
          <div className="video-check-options">
            <label><input type="checkbox" disabled={isProcessing} checked={preserveResolution} onChange={(event) => setPreserveResolution(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><span><strong>{t("preserveResolution")}</strong><small>{t("preserveResolutionHint")}</small></span></label>
            <label><input type="checkbox" disabled={isProcessing} checked={removeAudio} onChange={(event) => setRemoveAudio(event.target.checked)} /><span className="custom-check"><VolumeX size={13} /></span><span><strong>{t("removeAudio")}</strong><small>{t("removeAudioHint")}</small></span></label>
            <label><input type="checkbox" disabled={isProcessing} checked={preserveMetadata} onChange={(event) => setPreserveMetadata(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><span><strong>{t("preserveMetadata")}</strong><small>{t("preserveMetadataHint")}</small></span></label>
          </div>
          <div className="video-output-format">
            <strong>{t("outputFormat")}</strong>
            <div>{(["mp4", "webm"] as OutputFormat[]).map((value) => <button type="button" disabled={isProcessing} className={outputFormat === value ? "active" : ""} onClick={() => setOutputFormat(value)} key={value}>{value === "mp4" ? "MP4" : "WebM"}</button>)}</div>
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

      {activeProgress && <section className="video-progress-panel"><div><span>{t("videoProcessingFile", { current: batchPosition.current, total: batchPosition.total })}</span><strong>{activeProgress.name}</strong><b>{activeProgress.percent.toFixed(1)}%</b></div><progress max="100" value={activeProgress.percent} /><small>{formatDuration(activeProgress.processedSeconds)} / {formatDuration(activeProgress.durationSeconds)}</small></section>}
      {pageError && <div className="page-error"><AlertCircle size={16} />{pageError}</div>}
      {isProcessing ? <button className="run-button video-cancel-button" type="button" onClick={() => void cancelCompression()}><Ban size={19} />{t("cancelVideoCompression")}</button> : <button className="run-button video-compress-button" type="button" disabled={mode !== "recommended" || !readyItems.length} title={mode !== "recommended" ? t("videoModeNotImplemented") : undefined} onClick={() => void runRecommendedCompression()}><Clapperboard size={19} />{t("compressVideos")}</button>}
      {mode !== "recommended" && <p className="video-disabled-hint">{t("videoModeNotImplemented")}</p>}

      {previewResult && <div className="video-preview-backdrop" role="presentation" onMouseDown={(event) => { if (event.currentTarget === event.target) setPreviewResult(null); }}><section className="video-preview-modal" role="dialog" aria-modal="true" aria-label={t("previewVideo")}><header><div><strong>{previewResult.outputName}</strong><small>{previewResult.width} × {previewResult.height} · {previewResult.videoCodec} · {formatDuration(previewResult.durationSeconds)}</small></div><button className="icon-button" type="button" onClick={() => setPreviewResult(null)} aria-label={t("closePreview")}><X size={17} /></button></header><video src={convertFileSrc(previewResult.outputPath)} controls autoPlay /></section></div>}
    </div>
  );
}
