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
  FileVideo2,
  FolderOpen,
  ImageOff,
  LoaderCircle,
  Play,
  RefreshCw,
  Settings2,
  Trash2,
  UploadCloud,
  Video,
  X,
} from "lucide-react";
import { useI18n, type TranslationKey } from "../i18n";

type OutputFormat = "mp4" | "webm" | "mov" | "mkv";
type VideoCodec = "auto" | "openh264" | "svtAv1" | "copy";
type AudioCodec = "auto" | "aac" | "opus" | "copy" | "noAudio";
type ItemStatus = "analyzing" | "ready" | "waiting" | "converting" | "remuxing" | "completed" | "cancelled" | "failed";

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

interface ConversionPlan {
  valid: boolean;
  reason: string | null;
  plannedVideoCodec: string;
  plannedAudioCodec: string | null;
  remuxWithoutReencoding: boolean;
}

interface ConversionCapabilities {
  openh264: boolean;
  svtAv1: boolean;
  aac: boolean;
  opus: boolean;
  vp9: boolean;
  vorbis: boolean;
  webmMuxer: boolean;
  matroskaMuxer: boolean;
  av1Decoder: boolean;
  vp9Decoder: boolean;
  vp8Decoder: boolean;
}

interface VideoItem extends Partial<VideoInfo> {
  path: string;
  name: string;
  status: ItemStatus;
  thumbnailUrl?: string;
  errorCode?: string;
  errorMessage?: string;
  plan?: ConversionPlan;
  result?: ConversionResult;
}

interface ConversionResult {
  originalFormat: string;
  finalFormat: string;
  originalVideoCodec: string;
  finalVideoCodec: string;
  originalAudioCodec: string | null;
  finalAudioCodec: string | null;
  remuxWithoutReencoding: boolean;
  selectedQuality: number | null;
  width: number;
  height: number;
  framesPerSecond: number;
  originalSize: number;
  finalSize: number;
  processingDurationSeconds: number;
  outputPath: string;
  outputName: string;
}

interface ConversionProgress {
  path: string;
  name: string;
  processedSeconds: number;
  durationSeconds: number;
  percent: number;
  phase: "converting" | "remuxing";
}

const acceptedExtensions = ["mp4", "mov", "mkv", "avi", "webm", "m4v"];
const planReasonKeys: Record<string, TranslationKey> = {
  unsupported_video_codec: "convertVideoUnsupportedVideoCodec",
  unsupported_audio_codec: "convertVideoUnsupportedAudioCodec",
  no_audio_stream_to_copy: "convertVideoNoAudioToCopy",
  openh264_container_incompatible: "convertVideoOpenH264Incompatible",
  av1_container_incompatible: "convertVideoAv1Incompatible",
  copy_video_transform_incompatible: "convertVideoCopyTransformIncompatible",
  copy_video_container_incompatible: "convertVideoCopyVideoIncompatible",
  aac_container_incompatible: "convertVideoAacIncompatible",
  opus_container_incompatible: "convertVideoOpusIncompatible",
  copy_audio_container_incompatible: "convertVideoCopyAudioIncompatible",
};
const inspectErrorKeys: Record<string, TranslationKey> = {
  unsupported_video_format: "videoUnsupportedFormat",
  unsupported_video_file: "videoUnsupportedFile",
  invalid_video: "videoInvalidFile",
  video_not_found: "videoNotFound",
  video_read_error: "videoReadError",
  ffprobe_unavailable: "ffprobeUnavailable",
  ffmpeg_unavailable: "ffmpegUnavailable",
  inspection_failed: "videoInspectionFailed",
  conversion_format_not_implemented: "convertFormatNotImplemented",
  conversion_codec_not_implemented: "convertCodecNotImplemented",
  conversion_codec_unavailable: "convertCodecNotImplemented",
  incompatible_conversion_plan: "convertPlanUnavailable",
  invalid_custom_resolution: "convertResolutionInvalid",
  invalid_custom_frame_rate: "convertFrameRateInvalid",
  conversion_busy: "convertBusy",
  conversion_cancelled: "convertCancelledError",
  cancel_failed: "convertCancelFailed",
  destination_inaccessible: "videoDestinationInaccessible",
  disk_full: "videoDiskFull",
  conversion_failed: "convertFailedError",
  output_empty: "videoOutputEmpty",
  output_invalid: "videoOutputInvalid",
};

function fileName(path: string) {
  return path.split(/[\\/]/).pop() || path;
}

function formatBytes(bytes?: number) {
  if (!bytes) return "—";
  const units = ["B", "KB", "MB", "GB"];
  const unit = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** unit;
  return `${value >= 10 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
}

function formatDuration(seconds?: number) {
  if (seconds === undefined || !Number.isFinite(seconds)) return "—";
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${(seconds % 60).toFixed(1).padStart(4, "0")}`;
}

function parseError(error: unknown) {
  if (typeof error === "object" && error !== null) {
    const value = error as { code?: string; message?: string };
    return { code: value.code ?? "inspection_failed", message: value.message ?? "" };
  }
  try {
    return parseError(JSON.parse(String(error)));
  } catch {
    return { code: "inspection_failed", message: String(error) };
  }
}

function thumbnailUrl(data: number[] | null) {
  return data?.length ? URL.createObjectURL(new Blob([new Uint8Array(data)], { type: "image/png" })) : undefined;
}

function normalized(codec?: string | null) {
  return (codec ?? "").toUpperCase().replace(/-/g, "");
}

function videoCopySupported(format: OutputFormat, codec?: string) {
  const value = normalized(codec);
  if (format === "webm") return ["VP8", "VP9", "AV1"].includes(value);
  if (format === "mp4") return ["H264", "HEVC", "AV1", "MPEG4"].includes(value);
  if (format === "mov") return ["H264", "HEVC", "MPEG4", "PRORES"].includes(value);
  return !!value && value !== "UNKNOWN";
}

function audioCopySupported(format: OutputFormat, codec?: string | null) {
  const value = normalized(codec);
  if (!value) return false;
  if (format === "webm") return ["OPUS", "VORBIS"].includes(value);
  if (format === "mp4" || format === "mov") return ["AAC", "MP3", "ALAC", "OPUS"].includes(value);
  return value !== "UNKNOWN";
}

export function ConvertVideoPage() {
  const { t, language, setLanguage } = useI18n();
  const [items, setItems] = useState<VideoItem[]>([]);
  const [destination, setDestination] = useState<string | null>(null);
  const [outputFormat, setOutputFormat] = useState<OutputFormat>("mp4");
  const [videoCodec, setVideoCodec] = useState<VideoCodec>("auto");
  const [audioCodec, setAudioCodec] = useState<AudioCodec>("auto");
  const [preserveResolution, setPreserveResolution] = useState(true);
  const [preserveFrameRate, setPreserveFrameRate] = useState(true);
  const [preserveMetadata, setPreserveMetadata] = useState(false);
  const [quality, setQuality] = useState(75);
  const [customWidth, setCustomWidth] = useState("");
  const [customHeight, setCustomHeight] = useState("");
  const [customFrameRate, setCustomFrameRate] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [isProcessing, setIsProcessing] = useState(false);
  const [activeProgress, setActiveProgress] = useState<ConversionProgress | null>(null);
  const [batchPosition, setBatchPosition] = useState({ current: 0, total: 0 });
  const [previewResult, setPreviewResult] = useState<ConversionResult | null>(null);
  const [capabilities, setCapabilities] = useState<ConversionCapabilities | null>(null);
  const urls = useRef(new Set<string>());
  const cancellationRequested = useRef(false);

  const readyItems = items.filter((item) => item.status === "ready");
  const copyVideoAvailable = preserveResolution && preserveFrameRate
    && readyItems.length > 0
    && readyItems.every((item) => videoCopySupported(outputFormat, item.videoCodec));
  const copyAudioAvailable = readyItems.length > 0
    && readyItems.every((item) => item.audioCodec && audioCopySupported(outputFormat, item.audioCodec));
  const videoChoices = useMemo<VideoCodec[]>(() => {
    const choices: VideoCodec[] = ["auto"];
    if (capabilities?.openh264 && outputFormat !== "webm") choices.push("openh264");
    if (capabilities?.svtAv1 && (outputFormat === "webm" || outputFormat === "mkv")) choices.push("svtAv1");
    if (copyVideoAvailable) choices.push("copy");
    return choices;
  }, [capabilities, copyVideoAvailable, outputFormat]);
  const audioChoices = useMemo<AudioCodec[]>(() => {
    const choices: AudioCodec[] = ["auto"];
    if (capabilities?.aac && (outputFormat === "mp4" || outputFormat === "mov" || outputFormat === "mkv")) choices.push("aac");
    if (capabilities?.opus && (outputFormat === "webm" || outputFormat === "mkv")) choices.push("opus");
    if (copyAudioAvailable) choices.push("copy");
    choices.push("noAudio");
    return choices;
  }, [capabilities, copyAudioAvailable, outputFormat]);

  const outputFormats = useMemo<OutputFormat[]>(() => {
    const formats: OutputFormat[] = ["mp4", "mov"];
    if (capabilities?.webmMuxer && capabilities.svtAv1 && capabilities.opus) formats.splice(1, 0, "webm");
    if (capabilities?.matroskaMuxer) formats.push("mkv");
    return formats;
  }, [capabilities]);

  useEffect(() => {
    invoke<ConversionCapabilities>("get_video_conversion_capabilities")
      .then(setCapabilities)
      .catch((error) => setNotice(parseError(error).message || t("ffmpegUnavailable")));
  }, [t]);

  useEffect(() => {
    if (!outputFormats.includes(outputFormat)) setOutputFormat(outputFormats[0]);
  }, [outputFormat, outputFormats]);

  useEffect(() => {
    if (!videoChoices.includes(videoCodec)) setVideoCodec("auto");
  }, [videoChoices, videoCodec]);
  useEffect(() => {
    if (!audioChoices.includes(audioCodec)) setAudioCodec("auto");
  }, [audioChoices, audioCodec]);

  const addPaths = useCallback(async (paths: string[]) => {
    const unique = [...new Set(paths)].filter((path) =>
      acceptedExtensions.includes(path.split(".").pop()?.toLowerCase() ?? "")
      && !items.some((item) => item.path === path),
    );
    if (!unique.length) return;
    setNotice(null);
    setItems((current) => [...current, ...unique.map((path) => ({ path, name: fileName(path), status: "analyzing" as const }))]);
    for (const path of unique) {
      try {
        const info = await invoke<VideoInfo>("inspect_video", { path });
        const preview = thumbnailUrl(info.thumbnailData);
        if (preview) urls.current.add(preview);
        setItems((current) => current.map((item) => item.path === path ? { ...info, status: "ready", thumbnailUrl: preview } : item));
      } catch (error) {
        const parsed = parseError(error);
        setItems((current) => current.map((item) => item.path === path ? { ...item, status: "failed", errorCode: parsed.code, errorMessage: parsed.message } : item));
      }
    }
  }, [items]);

  useEffect(() => {
    let disposed = false;
    let cleanup: (() => void) | undefined;
    getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "drop") void addPaths(event.payload.paths);
    }).then((unlisten) => { if (disposed) unlisten(); else cleanup = unlisten; });
    return () => { disposed = true; cleanup?.(); };
  }, [addPaths]);

  useEffect(() => () => {
    urls.current.forEach(URL.revokeObjectURL);
    urls.current.clear();
  }, []);

  useEffect(() => {
    let disposed = false;
    let cleanup: (() => void) | undefined;
    listen<ConversionProgress>("video-conversion-progress", (event) => {
      if (disposed) return;
      setActiveProgress(event.payload);
      setItems((current) => current.map((item) => item.path === event.payload.path ? {
        ...item,
        status: event.payload.phase === "remuxing" ? "remuxing" : "converting",
      } : item));
    }).then((unlisten) => { if (disposed) unlisten(); else cleanup = unlisten; });
    return () => { disposed = true; cleanup?.(); };
  }, []);

  useEffect(() => {
    let disposed = false;
    Promise.all(readyItems.map(async (item) => ({
      path: item.path,
      plan: await invoke<ConversionPlan>("plan_video_conversion", {
        request: {
          inputVideoCodec: item.videoCodec,
          inputAudioCodec: item.audioCodec ?? null,
          outputFormat,
          videoCodec,
          audioCodec,
          preserveResolution,
          preserveFrameRate,
        },
      }),
    }))).then((plans) => {
      if (!disposed) setItems((current) => current.map((item) => {
        const planned = plans.find((entry) => entry.path === item.path);
        return planned ? { ...item, plan: planned.plan } : item;
      }));
    }).catch(() => undefined);
    return () => { disposed = true; };
  }, [audioCodec, outputFormat, preserveFrameRate, preserveResolution, videoCodec, readyItems.map((item) => item.path).join("|")]);

  async function chooseVideos() {
    if (isProcessing) return;
    const selected = await open({ multiple: true, directory: false, filters: [{ name: "MP4, MOV, MKV, AVI, WebM, M4V", extensions: acceptedExtensions }] });
    if (selected) await addPaths(Array.isArray(selected) ? selected : [selected]);
  }

  function removeItem(path: string) {
    if (isProcessing) return;
    setItems((current) => {
      const item = current.find((entry) => entry.path === path);
      if (item?.thumbnailUrl) { URL.revokeObjectURL(item.thumbnailUrl); urls.current.delete(item.thumbnailUrl); }
      return current.filter((entry) => entry.path !== path);
    });
  }

  function clearAll() {
    if (isProcessing) return;
    urls.current.forEach(URL.revokeObjectURL);
    urls.current.clear();
    setItems([]);
    setNotice(null);
  }

  const width = customWidth.trim() ? Number(customWidth) : null;
  const height = customHeight.trim() ? Number(customHeight) : null;
  const fps = Number(customFrameRate);
  const validDimension = (value: number | null) => value === null || (Number.isInteger(value) && value >= 2);
  const resolutionValid = preserveResolution || ((width !== null || height !== null) && validDimension(width) && validDimension(height));
  const frameRateValid = preserveFrameRate || (Number.isFinite(fps) && fps > 0 && fps <= 240);
  const plansReady = readyItems.length > 0 && readyItems.every((item) => item.plan?.valid);
  const canConvert = capabilities !== null && plansReady && resolutionValid && frameRateValid && !isProcessing;
  const codecLabel = (codec: VideoCodec | AudioCodec, kind: "video" | "audio") => t(({
    auto: "convertVideoCodecAuto",
    openh264: "convertVideoCodecOpenH264",
    svtAv1: "convertVideoCodecSvtAv1",
    copy: kind === "video" ? "convertVideoCodecCopy" : "convertAudioCodecCopy",
    aac: "convertAudioCodecAac",
    opus: "convertAudioCodecOpus",
    noAudio: "convertAudioCodecNone",
  } as Record<string, TranslationKey>)[codec]);

  function plannedResolution(item: VideoItem) {
    if (preserveResolution || !resolutionValid || !item.width || !item.height) return `${item.width} × ${item.height}`;
    const widthScale = width === null ? Number.POSITIVE_INFINITY : width / item.width;
    const heightScale = height === null ? Number.POSITIVE_INFINITY : height / item.height;
    const scale = Math.min(widthScale, heightScale, 1);
    const even = (value: number) => Math.max(2, Math.floor(value) - Math.floor(value) % 2);
    return `${even(item.width * scale)} × ${even(item.height * scale)}`;
  }

  async function runConversion() {
    const queue = items.filter((item) => item.status === "ready" && item.plan?.valid);
    if (!queue.length || !canConvert) return;
    setIsProcessing(true);
    setNotice(null);
    setBatchPosition({ current: 1, total: queue.length });
    cancellationRequested.current = false;
    const paths = new Set(queue.map((item) => item.path));
    setItems((current) => current.map((item) => paths.has(item.path) ? { ...item, status: "waiting", errorCode: undefined, errorMessage: undefined } : item));

    for (let index = 0; index < queue.length; index += 1) {
      const item = queue[index];
      if (cancellationRequested.current) break;
      setBatchPosition({ current: index + 1, total: queue.length });
      const phase = item.plan?.remuxWithoutReencoding ? "remuxing" : "converting";
      setActiveProgress({ path: item.path, name: item.name, processedSeconds: 0, durationSeconds: item.durationSeconds ?? 0, percent: 0, phase });
      setItems((current) => current.map((entry) => entry.path === item.path ? { ...entry, status: phase } : entry));
      try {
        const result = await invoke<ConversionResult>("convert_video", {
          request: {
            path: item.path,
            destinationDir: destination,
            outputFormat,
            videoCodec,
            audioCodec,
            preserveResolution,
            preserveFrameRate,
            preserveMetadata,
            quality,
            customWidth: preserveResolution ? null : width,
            customHeight: preserveResolution ? null : height,
            customFrameRate: preserveFrameRate ? null : fps,
          },
        });
        setItems((current) => current.map((entry) => entry.path === item.path ? { ...entry, status: "completed", result } : entry));
      } catch (error) {
        const parsed = parseError(error);
        const cancelled = parsed.code === "conversion_cancelled";
        setItems((current) => current.map((entry) => entry.path === item.path ? { ...entry, status: cancelled ? "cancelled" : "failed", errorCode: parsed.code, errorMessage: parsed.message } : entry));
        if (cancelled || cancellationRequested.current) break;
      }
    }
    if (cancellationRequested.current) {
      setItems((current) => current.map((item) => item.status === "waiting" ? { ...item, status: "cancelled", errorCode: "conversion_cancelled" } : item));
    }
    setActiveProgress(null);
    setIsProcessing(false);
    cancellationRequested.current = false;
  }

  async function cancelConversion() {
    cancellationRequested.current = true;
    try {
      await invoke("cancel_video_conversion");
    } catch (error) {
      const parsed = parseError(error);
      setNotice(parsed.message || t("convertCancelFailed"));
    }
  }

  return <div className="page convert-video-page">
    <header className="page-header compression-header video-header">
      <div><span className="eyebrow">{t("convertVideoModuleLabel")}</span><h1>{t("convertVideoTitle")}</h1><p>{t("convertVideoSubtitle")}</p></div>
      <div className="compress-header-actions"><label className="language-select"><span>{language.toUpperCase()}</span><select value={language} onChange={(event) => setLanguage(event.target.value as "en" | "fr" | "ar")}><option value="en">English</option><option value="fr">Français</option><option value="ar">العربية</option></select></label><div className="header-status"><Clapperboard size={17} /><span>FFprobe</span></div></div>
    </header>

    <section className={`video-drop-zone convert-video-drop ${isProcessing ? "disabled" : ""}`} onClick={() => void chooseVideos()}>
      <div className="video-drop-icon"><UploadCloud size={34} /></div><div><h2>{t("convertVideoDropTitle")}</h2><p>{t("convertVideoDropHint")}</p></div>
      <button className="primary-button" disabled={isProcessing} type="button" onClick={(event) => { event.stopPropagation(); void chooseVideos(); }}><FileVideo2 size={17} />{t("addVideos")}</button>
    </section>

    <section className="compress-panel convert-video-files">
      <div className="compress-panel-header"><div><h2>{t("selectedVideos")}</h2><span>{t("videoCount", { count: items.length })}</span></div>{!!items.length && <button className="text-button danger" disabled={isProcessing} type="button" onClick={clearAll}><Trash2 size={14} />{t("clearAll")}</button>}</div>
      {!items.length ? <div className="video-empty-state"><RefreshCw size={30} /><strong>{t("convertVideoEmpty")}</strong><p>{t("convertVideoEmptyHint")}</p></div> : <div className="convert-video-list">{items.map((item) => <article className={`convert-video-row ${item.status}`} key={item.path}>
        <div className="video-thumbnail">{item.thumbnailUrl ? <img src={item.thumbnailUrl} alt="" /> : item.status === "analyzing" ? <LoaderCircle className="spin" size={22} /> : <ImageOff size={21} />}</div>
        <div className="convert-video-file"><div className="video-file-title"><div><strong>{item.name}</strong><small title={item.path}>{item.path}</small></div><div className={`video-status ${item.status}`}>{item.status === "analyzing" || item.status === "converting" || item.status === "remuxing" ? <LoaderCircle className="spin" size={13} /> : item.status === "ready" || item.status === "completed" ? <CheckCircle2 size={13} /> : item.status === "cancelled" ? <Ban size={13} /> : item.status === "waiting" ? <RefreshCw size={13} /> : <AlertCircle size={13} />}<span>{t(item.status === "analyzing" ? "analyzingVideo" : item.status === "ready" ? "videoReady" : item.status === "waiting" ? "convertWaiting" : item.status === "converting" ? "convertConverting" : item.status === "remuxing" ? "convertRemuxing" : item.status === "completed" ? "videoCompleted" : item.status === "cancelled" ? "videoCancelled" : "videoFailed")}</span></div></div>
          {item.status === "failed" || item.status === "cancelled" ? <p className="video-row-error"><AlertCircle size={14} />{t(inspectErrorKeys[item.errorCode ?? ""] ?? "videoInspectionFailed")}</p> : <div className="convert-video-metadata"><span><small>{t("format")}</small><strong>{item.format ?? "—"}</strong></span><span><small>{t("videoDuration")}</small><strong>{formatDuration(item.durationSeconds)}</strong></span><span><small>{t("resolution")}</small><strong>{item.width} × {item.height}</strong></span><span><small>{t("fileSize")}</small><strong>{formatBytes(item.size)}</strong></span><span><small>{t("videoCodec")}</small><strong>{item.videoCodec}</strong></span><span><small>{t("audioCodec")}</small><strong>{item.audioCodec ?? t("noAudio")}</strong></span><span><small>{t("convertVideoFrameRate")}</small><strong>{item.framesPerSecond?.toFixed(3)} fps</strong></span></div>}
          {item.result && <div className="convert-result"><div className="convert-result-mode"><CheckCircle2 size={14} />{t(item.result.remuxWithoutReencoding ? "convertRemux" : "convertReencode")}</div><div className="convert-result-grid"><span><small>{t("convertOriginalFormat")}</small><strong>{item.result.originalFormat}</strong></span><span><small>{t("convertFinalFormat")}</small><strong>{item.result.finalFormat}</strong></span><span><small>{t("convertOriginalVideoCodec")}</small><strong>{item.result.originalVideoCodec}</strong></span><span><small>{t("finalCodec")}</small><strong>{item.result.finalVideoCodec}</strong></span><span><small>{t("convertOriginalAudioCodec")}</small><strong>{item.result.originalAudioCodec ?? t("noAudio")}</strong></span><span><small>{t("convertPlannedAudioCodec")}</small><strong>{item.result.finalAudioCodec ?? t("noAudio")}</strong></span><span><small>{t("finalResolution")}</small><strong>{item.result.width} × {item.result.height}</strong></span><span><small>{t("convertVideoFrameRate")}</small><strong>{item.result.framesPerSecond.toFixed(3)} fps</strong></span><span><small>{t("originalSize")}</small><strong>{formatBytes(item.result.originalSize)}</strong></span><span><small>{t("convertFinalSize")}</small><strong>{formatBytes(item.result.finalSize)}</strong></span><span><small>{t("processingDuration")}</small><strong>{formatDuration(item.result.processingDurationSeconds)}</strong></span>{item.result.selectedQuality !== null && <span><small>{t("convertVideoQuality")}</small><strong>{item.result.selectedQuality}</strong></span>}</div><div className="video-result-location"><FolderOpen size={13} /><span title={item.result.outputPath}>{item.result.outputPath}</span></div><div className="video-result-actions"><button className="text-button" type="button" onClick={() => setPreviewResult(item.result!)}><Play size={13} />{t("previewVideo")}</button><button className="text-button" type="button" onClick={() => void openPath(item.result!.outputPath)}><Video size={13} />{t("openVideo")}</button><button className="text-button" type="button" onClick={() => void revealItemInDir(item.result!.outputPath)}><FolderOpen size={13} />{t("openFolder")}</button></div></div>}
        </div><button className="video-remove-button" disabled={isProcessing} type="button" onClick={() => removeItem(item.path)}><Trash2 size={15} /><span>{t("remove")}</span></button>
      </article>)}</div>}
    </section>

    <div className={`convert-video-settings-grid ${isProcessing ? "disabled" : ""}`}>
      <section className="compress-panel convert-video-format-panel"><div className="video-section-title"><RefreshCw size={18} /><div><h2>{t("convertOutputFormat")}</h2><p>{t("convertOutputFormatHint")}</p></div></div><div className="convert-format-options">{outputFormats.map((format) => <button type="button" className={outputFormat === format ? "active" : ""} key={format} onClick={() => setOutputFormat(format)}>{format === "webm" ? "WebM" : format.toUpperCase()}</button>)}</div>
        <label className="convert-select"><span>{t("convertVideoCodec")}</span><select value={videoCodec} onChange={(event) => setVideoCodec(event.target.value as VideoCodec)}>{videoChoices.map((codec) => <option value={codec} key={codec}>{codecLabel(codec, "video")}</option>)}</select></label>
        <label className="convert-select"><span>{t("convertAudioCodec")}</span><select value={audioCodec} onChange={(event) => setAudioCodec(event.target.value as AudioCodec)}>{audioChoices.map((codec) => <option value={codec} key={codec}>{codecLabel(codec, "audio")}</option>)}</select></label>
        <label className="convert-quality"><span><strong>{t("convertVideoQuality")}</strong><b>{quality}</b></span><input type="range" min="1" max="100" value={quality} onChange={(event) => setQuality(Number(event.target.value))} /></label>
      </section>

      <section className="compress-panel convert-video-options-panel"><div className="video-section-title"><Settings2 size={18} /><div><h2>{t("videoOptions")}</h2><p>{t("convertVideoOptionsHint")}</p></div></div>
        <div className="convert-checks">{[["preserveResolution", preserveResolution, setPreserveResolution], ["convertPreserveFrameRate", preserveFrameRate, setPreserveFrameRate], ["preserveMetadata", preserveMetadata, setPreserveMetadata]].map(([key, checked, setter]) => <label key={String(key)}><input type="checkbox" checked={checked as boolean} onChange={(event) => (setter as (value: boolean) => void)(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><strong>{t(key as TranslationKey)}</strong></label>)}</div>
        {!preserveResolution && <div className="convert-custom-fields"><label><span>{t("convertCustomWidth")}</span><input type="number" min="2" step="2" value={customWidth} onChange={(event) => setCustomWidth(event.target.value)} /></label><label><span>{t("convertCustomHeight")}</span><input type="number" min="2" step="2" value={customHeight} onChange={(event) => setCustomHeight(event.target.value)} /></label></div>}
        {!resolutionValid && <p className="convert-validation"><AlertCircle size={13} />{t("convertResolutionInvalid")}</p>}
        {!preserveFrameRate && <label className="convert-custom-single"><span>{t("convertCustomFrameRate")}</span><input type="number" min="0.001" max="240" step="0.001" value={customFrameRate} onChange={(event) => setCustomFrameRate(event.target.value)} /></label>}
        {!frameRateValid && <p className="convert-validation"><AlertCircle size={13} />{t("convertFrameRateInvalid")}</p>}
      </section>
    </div>

    {!!readyItems.length && <section className="compress-panel convert-plan-panel"><div className="video-section-title"><Clapperboard size={18} /><div><h2>{t("convertPlanTitle")}</h2><p>{t("convertPlanHint")}</p></div></div><div className="convert-plan-list">{readyItems.map((item) => <article className={`convert-plan ${item.plan?.valid ? "valid" : "invalid"}`} key={item.path}><header><strong>{item.name}</strong>{item.plan?.valid ? <span><CheckCircle2 size={13} />{t(item.plan.remuxWithoutReencoding ? "convertRemux" : "convertReencode")}</span> : <span><AlertCircle size={13} />{t(planReasonKeys[item.plan?.reason ?? ""] ?? "convertPlanUnavailable")}</span>}</header><div><span><small>{t("convertOriginalFormat")}</small><strong>{item.format}</strong></span><span><small>{t("convertFinalFormat")}</small><strong>{outputFormat === "webm" ? "WebM" : outputFormat.toUpperCase()}</strong></span><span><small>{t("convertOriginalVideoCodec")}</small><strong>{item.videoCodec}</strong></span><span><small>{t("convertPlannedVideoCodec")}</small><strong>{item.plan?.plannedVideoCodec || "—"}</strong></span><span><small>{t("convertOriginalAudioCodec")}</small><strong>{item.audioCodec ?? t("noAudio")}</strong></span><span><small>{t("convertPlannedAudioCodec")}</small><strong>{item.plan?.plannedAudioCodec ?? t("noAudio")}</strong></span><span><small>{t("convertPlannedResolution")}</small><strong>{resolutionValid ? plannedResolution(item) : "—"}</strong></span><span><small>{t("convertPlannedFrameRate")}</small><strong>{preserveFrameRate ? `${item.framesPerSecond?.toFixed(3)} fps` : frameRateValid ? `${fps.toFixed(3)} fps` : "—"}</strong></span></div></article>)}</div></section>}

    <section className="compress-panel video-destination-panel"><div><FolderOpen size={19} /><span><strong>{t("destinationFolder")}</strong><small title={destination ?? ""}>{destination ?? t("videoAutomaticDestination")}</small></span></div><button className="secondary-button" disabled={isProcessing} type="button" onClick={async () => { const selected = await open({ directory: true, multiple: false }); if (typeof selected === "string") setDestination(selected); }}>{t("chooseFolder")}</button></section>
    {notice && <p className="convert-interface-notice"><CheckCircle2 size={14} />{notice}</p>}
    {activeProgress && <section className="video-progress-panel convert-progress"><div><span>{t(activeProgress.phase === "remuxing" ? "convertRemuxing" : "convertConverting")}</span><strong>{activeProgress.name}</strong><b>{activeProgress.percent.toFixed(1)}%</b></div><progress max="100" value={activeProgress.percent} /><small>{t("videoProcessingFile", { current: batchPosition.current, total: batchPosition.total })} · {formatDuration(activeProgress.processedSeconds)} / {formatDuration(activeProgress.durationSeconds)}</small></section>}
    {isProcessing ? <button className="run-button video-cancel-button" type="button" onClick={() => void cancelConversion()}><Ban size={19} />{t("convertCancel")}</button> : <button className="run-button convert-video-button" type="button" disabled={!canConvert} onClick={() => void runConversion()}><RefreshCw size={19} />{t("convertVideos")}</button>}
    {previewResult && <div className="video-preview-backdrop" role="presentation" onMouseDown={(event) => { if (event.currentTarget === event.target) setPreviewResult(null); }}><section className="video-preview-modal" role="dialog" aria-modal="true" aria-label={t("previewVideo")}><header><div><strong>{previewResult.outputName}</strong><small>{previewResult.width} × {previewResult.height} · {previewResult.finalVideoCodec}</small></div><button className="icon-button" type="button" onClick={() => setPreviewResult(null)}><X size={17} /></button></header><video src={convertFileSrc(previewResult.outputPath)} controls autoPlay /></section></div>}
  </div>;
}
