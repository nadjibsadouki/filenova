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
  FileAudio2,
  FolderOpen,
  ListMusic,
  LoaderCircle,
  Music2,
  Play,
  Settings2,
  Trash2,
  UploadCloud,
  X,
} from "lucide-react";
import { useI18n, type TranslationKey } from "../i18n";

type OutputFormat = "mp3" | "wav" | "m4a" | "flac" | "ogg";
type Quality = "low" | "standard" | "high" | "maximum";
type ItemStatus = "analyzing" | "ready" | "noAudio" | "waiting" | "extracting" | "copying" | "completed" | "cancelled" | "failed";

interface AudioCapabilities {
  mp3: boolean;
  wav: boolean;
  m4a: boolean;
  flac: boolean;
  oggOpus: boolean;
}

interface AudioSourceInfo {
  path: string;
  name: string;
  format: string;
  durationSeconds: number;
  size: number;
  audioCodec: string | null;
  audioBitrate: number | null;
  sampleRate: number | null;
  channels: number | null;
  audioTracks: AudioTrack[];
}

interface AudioTrack {
  audioIndex: number;
  streamIndex: number;
  language: string | null;
  title: string | null;
  codec: string;
  bitrate: number | null;
  sampleRate: number | null;
  channels: number | null;
}

interface AudioPlan {
  valid: boolean;
  reason: string | null;
  plannedCodec: string | null;
  plannedBitrate: number | null;
  plannedSampleRate: number | null;
  plannedChannels: number | null;
  extractWithoutReencoding: boolean;
}

interface AudioItem extends Partial<AudioSourceInfo> {
  path: string;
  name: string;
  status: ItemStatus;
  errorCode?: string;
  plan?: AudioPlan;
  selectedTrack: number;
  result?: AudioResult;
}

interface AudioResult {
  sourceName: string;
  finalFormat: string;
  finalCodec: string;
  finalBitrate: number | null;
  sampleRate: number | null;
  channels: number | null;
  selectedAudioTrack: number;
  selectedAudioLanguage: string | null;
  extractWithoutReencoding: boolean;
  finalSize: number;
  processingDurationSeconds: number;
  outputPath: string;
  outputName: string;
}

interface AudioProgress {
  path: string;
  name: string;
  processedSeconds: number;
  durationSeconds: number;
  percent: number;
  phase: "extracting" | "copying";
}

const extensions = ["mp4", "mov", "mkv", "avi", "webm", "m4v"];
const bitrates = [64, 96, 128, 192, 256, 320];
const planReasons: Record<string, TranslationKey> = {
  no_audio_stream: "extractAudioNoTrack",
  unsupported_output_format: "extractAudioUnsupportedOutput",
  invalid_sample_rate: "extractAudioInvalidSampleRate",
  invalid_channels: "extractAudioInvalidChannels",
  copy_codec_incompatible: "extractAudioCopyCodecIncompatible",
  copy_transform_incompatible: "extractAudioCopyTransformIncompatible",
};
const runErrors: Record<string, TranslationKey> = {
  no_audio_stream: "extractAudioNoTrack",
  audio_track_not_found: "extractAudioTrackMissing",
  codec_unsupported: "extractAudioCodecUnsupported",
  incompatible_audio_plan: "extractAudioCopyCodecIncompatible",
  ffmpeg_unavailable: "ffmpegUnavailable",
  ffprobe_unavailable: "ffprobeUnavailable",
  destination_inaccessible: "videoDestinationInaccessible",
  disk_full: "videoDiskFull",
  extraction_cancelled: "extractAudioCancelledError",
  output_empty: "videoOutputEmpty",
  output_invalid: "videoOutputInvalid",
  extraction_failed: "extractAudioFailedError",
};

function nameFromPath(path: string) {
  return path.split(/[\\/]/).pop() || path;
}

function formatBytes(bytes?: number | null) {
  if (!bytes) return "—";
  const units = ["B", "KB", "MB", "GB"];
  const unit = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** unit;
  return `${value >= 10 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
}

function duration(seconds?: number) {
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

function copyCompatible(format: OutputFormat, codec?: string | null) {
  const value = (codec ?? "").toUpperCase();
  if (format === "mp3") return value === "MP3";
  if (format === "wav") return value.startsWith("PCM_");
  if (format === "m4a") return value === "AAC";
  if (format === "flac") return value === "FLAC";
  return value === "OPUS";
}

export function ExtractAudioPage() {
  const { t, language, setLanguage } = useI18n();
  const [items, setItems] = useState<AudioItem[]>([]);
  const [capabilities, setCapabilities] = useState<AudioCapabilities | null>(null);
  const [outputFormat, setOutputFormat] = useState<OutputFormat>("mp3");
  const [quality, setQuality] = useState<Quality>("standard");
  const [bitrate, setBitrate] = useState<number | null>(null);
  const [sampleRate, setSampleRate] = useState("original");
  const [customSampleRate, setCustomSampleRate] = useState("");
  const [channels, setChannels] = useState("original");
  const [preserveMetadata, setPreserveMetadata] = useState(false);
  const [copyAudio, setCopyAudio] = useState(false);
  const [destination, setDestination] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [isProcessing, setIsProcessing] = useState(false);
  const [activeProgress, setActiveProgress] = useState<AudioProgress | null>(null);
  const [batchPosition, setBatchPosition] = useState({ current: 0, total: 0 });
  const [previewResult, setPreviewResult] = useState<AudioResult | null>(null);
  const cancellationRequested = useRef(false);
  const itemsRef = useRef(items);
  itemsRef.current = items;

  const outputFormats = useMemo(() => {
    if (!capabilities) return [] as OutputFormat[];
    return (["mp3", "wav", "m4a", "flac", "ogg"] as OutputFormat[]).filter((format) => (
      format === "ogg" ? capabilities.oggOpus : capabilities[format]
    ));
  }, [capabilities]);
  const validItems = items.filter((item) => item.status === "ready");
  const copyAvailable = validItems.length > 0
    && validItems.every((item) => copyCompatible(outputFormat, item.audioTracks?.[item.selectedTrack]?.codec));
  const lossyFormat = ["mp3", "m4a", "ogg"].includes(outputFormat);
  const requestedSampleRate = sampleRate === "original"
    ? null
    : sampleRate === "custom"
      ? Number(customSampleRate)
      : Number(sampleRate);
  const sampleRateValid = sampleRate !== "custom"
    || (requestedSampleRate !== null
      && Number.isInteger(requestedSampleRate)
      && requestedSampleRate >= 8_000
      && requestedSampleRate <= 192_000);
  const plansValid = validItems.length > 0 && validItems.every((item) => item.plan?.valid);
  const canExtract = plansValid && sampleRateValid && outputFormats.includes(outputFormat) && !isProcessing;

  useEffect(() => {
    invoke<AudioCapabilities>("get_audio_extraction_capabilities")
      .then(setCapabilities)
      .catch((error) => setNotice(parseError(error).message || t("ffmpegUnavailable")));
  }, [t]);

  useEffect(() => {
    if (outputFormats.length && !outputFormats.includes(outputFormat)) {
      setOutputFormat(outputFormats[0]);
    }
  }, [outputFormat, outputFormats]);

  const addPaths = useCallback(async (paths: string[]) => {
    const selected = [...new Set(paths)].filter((path) => (
      extensions.includes(path.split(".").pop()?.toLowerCase() ?? "")
      && !itemsRef.current.some((item) => item.path === path)
    ));
    if (!selected.length) return;
    setNotice(null);
    setItems((current) => [...current, ...selected.map((path) => ({
      path,
      name: nameFromPath(path),
      status: "analyzing" as const,
      selectedTrack: 0,
    }))]);
    for (const path of selected) {
      try {
        const info = await invoke<AudioSourceInfo>("inspect_audio_source", { path });
        setItems((current) => current.map((item) => item.path === path
          ? { ...info, status: info.audioCodec ? "ready" : "noAudio", selectedTrack: 0 }
          : item));
      } catch (error) {
        const parsed = parseError(error);
        setItems((current) => current.map((item) => item.path === path
          ? { ...item, status: "failed", errorCode: parsed.code }
          : item));
      }
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    let cleanup: (() => void) | undefined;
    getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "drop") void addPaths(event.payload.paths);
    }).then((unlisten) => {
      if (disposed) unlisten(); else cleanup = unlisten;
    });
    return () => { disposed = true; cleanup?.(); };
  }, [addPaths]);

  useEffect(() => {
    let disposed = false;
    let cleanup: (() => void) | undefined;
    listen<AudioProgress>("audio-extraction-progress", (event) => {
      if (disposed) return;
      setActiveProgress(event.payload);
      setItems((current) => current.map((item) => item.path === event.payload.path
        ? { ...item, status: event.payload.phase }
        : item));
    }).then((unlisten) => {
      if (disposed) unlisten(); else cleanup = unlisten;
    });
    return () => { disposed = true; cleanup?.(); };
  }, []);

  useEffect(() => {
    let disposed = false;
    Promise.all(validItems.map(async (item) => ({
      path: item.path,
      plan: await invoke<AudioPlan>("plan_audio_extraction", {
        request: {
          inputAudioCodec: item.audioTracks?.[item.selectedTrack]?.codec ?? null,
          inputAudioBitrate: item.audioTracks?.[item.selectedTrack]?.bitrate ?? null,
          inputSampleRate: item.audioTracks?.[item.selectedTrack]?.sampleRate ?? null,
          inputChannels: item.audioTracks?.[item.selectedTrack]?.channels ?? null,
          outputFormat,
          quality,
          customBitrate: lossyFormat ? bitrate : null,
          sampleRate: sampleRateValid ? requestedSampleRate : 1,
          channels: channels === "original" ? null : Number(channels),
          copyAudioStream: copyAudio,
        },
      }),
    }))).then((plans) => {
      if (!disposed) setItems((current) => current.map((item) => {
        const entry = plans.find((plan) => plan.path === item.path);
        return entry ? { ...item, plan: entry.plan } : item;
      }));
    }).catch(() => undefined);
    return () => { disposed = true; };
  }, [bitrate, channels, copyAudio, lossyFormat, outputFormat, quality, requestedSampleRate, sampleRateValid, validItems.map((item) => `${item.path}:${item.selectedTrack}`).join("|")]);

  async function chooseVideos() {
    const selected = await open({
      multiple: true,
      directory: false,
      filters: [{ name: "MP4, MOV, MKV, AVI, WebM, M4V", extensions }],
    });
    if (selected) await addPaths(Array.isArray(selected) ? selected : [selected]);
  }

  function remove(path: string) {
    setItems((current) => current.filter((item) => item.path !== path));
  }

  const qualityLabel = (value: Quality) => t(({
    low: "extractAudioQualityLow",
    standard: "extractAudioQualityStandard",
    high: "extractAudioQualityHigh",
    maximum: "extractAudioQualityMaximum",
  } as const)[value]);
  const statusLabel = (status: ItemStatus) => t(({
    analyzing: "analyzingVideo",
    ready: "videoReady",
    noAudio: "extractAudioNoTrack",
    waiting: "extractAudioWaiting",
    extracting: "extractAudioExtracting",
    copying: "extractAudioCopying",
    completed: "videoCompleted",
    cancelled: "videoCancelled",
    failed: "videoFailed",
  } as Record<ItemStatus, TranslationKey>)[status]);
  const errorLabel = (item: AudioItem) => t(runErrors[item.errorCode ?? ""] ?? "videoInspectionFailed");

  async function runExtraction() {
    const queue = items.filter((item) => item.status === "ready" && item.plan?.valid);
    if (!queue.length || !canExtract) return;
    setIsProcessing(true);
    setNotice(null);
    setBatchPosition({ current: 1, total: queue.length });
    cancellationRequested.current = false;
    const paths = new Set(queue.map((item) => item.path));
    setItems((current) => current.map((item) => paths.has(item.path)
      ? { ...item, status: "waiting", errorCode: undefined }
      : item));
    for (let index = 0; index < queue.length; index += 1) {
      const item = queue[index];
      if (cancellationRequested.current) break;
      setBatchPosition({ current: index + 1, total: queue.length });
      const phase = item.plan?.extractWithoutReencoding ? "copying" : "extracting";
      setActiveProgress({
        path: item.path,
        name: item.name,
        processedSeconds: 0,
        durationSeconds: item.durationSeconds ?? 0,
        percent: 0,
        phase,
      });
      setItems((current) => current.map((entry) => entry.path === item.path
        ? { ...entry, status: phase }
        : entry));
      try {
        const result = await invoke<AudioResult>("extract_audio", {
          request: {
            path: item.path,
            destinationDir: destination,
            outputFormat,
            quality,
            customBitrate: lossyFormat ? bitrate : null,
            sampleRate: requestedSampleRate,
            channels: channels === "original" ? null : Number(channels),
            preserveMetadata,
            copyAudioStream: copyAudio,
            selectedAudioTrack: item.selectedTrack,
          },
        });
        setItems((current) => current.map((entry) => entry.path === item.path
          ? { ...entry, status: "completed", result }
          : entry));
      } catch (error) {
        const parsed = parseError(error);
        const cancelled = parsed.code === "extraction_cancelled";
        setItems((current) => current.map((entry) => entry.path === item.path
          ? { ...entry, status: cancelled ? "cancelled" : "failed", errorCode: parsed.code }
          : entry));
        if (cancelled || cancellationRequested.current) break;
      }
    }
    if (cancellationRequested.current) {
      setItems((current) => current.map((item) => item.status === "waiting"
        ? { ...item, status: "cancelled", errorCode: "extraction_cancelled" }
        : item));
    }
    setActiveProgress(null);
    setIsProcessing(false);
    cancellationRequested.current = false;
  }

  async function cancelExtraction() {
    cancellationRequested.current = true;
    try {
      await invoke("cancel_audio_extraction");
    } catch (error) {
      const parsed = parseError(error);
      setNotice(parsed.message || t("extractAudioCancelFailed"));
    }
  }

  return <div className="page extract-audio-page">
    <header className="page-header compression-header video-header">
      <div><span className="eyebrow">{t("extractAudioModuleLabel")}</span><h1>{t("extractAudioTitle")}</h1><p>{t("extractAudioSubtitle")}</p></div>
      <div className="compress-header-actions"><label className="language-select"><span>{language.toUpperCase()}</span><select value={language} onChange={(event) => setLanguage(event.target.value as "en" | "fr" | "ar")}><option value="en">English</option><option value="fr">Français</option><option value="ar">العربية</option></select></label><div className="header-status"><Music2 size={17} /><span>FFprobe</span></div></div>
    </header>

    <section className={`video-drop-zone extract-audio-drop ${isProcessing ? "disabled" : ""}`} onClick={() => { if (!isProcessing) void chooseVideos(); }}>
      <div className="video-drop-icon"><UploadCloud size={34} /></div><div><h2>{t("extractAudioDropTitle")}</h2><p>{t("extractAudioDropHint")}</p></div>
      <button className="primary-button" disabled={isProcessing} type="button" onClick={(event) => { event.stopPropagation(); void chooseVideos(); }}><FileAudio2 size={17} />{t("addVideos")}</button>
    </section>

    <section className="compress-panel extract-audio-files">
      <div className="compress-panel-header"><div><h2>{t("selectedVideos")}</h2><span>{t("videoCount", { count: items.length })}</span></div>{!!items.length && <button className="text-button danger" disabled={isProcessing} type="button" onClick={() => setItems([])}><Trash2 size={14} />{t("clearAll")}</button>}</div>
      {!items.length ? <div className="video-empty-state"><ListMusic size={30} /><strong>{t("extractAudioEmpty")}</strong><p>{t("extractAudioEmptyHint")}</p></div> : <div className="extract-audio-list">{items.map((item) => <article className={`extract-audio-row ${item.status}`} key={item.path}>
        <div className="extract-audio-icon">{["analyzing", "waiting", "extracting", "copying"].includes(item.status) ? <LoaderCircle className="spin" size={22} /> : <FileAudio2 size={22} />}</div><div className="extract-audio-file"><div className="video-file-title"><div><strong>{item.name}</strong><small title={item.path}>{item.path}</small></div><div className={`video-status ${item.status}`}><span>{statusLabel(item.status)}</span></div></div>
          {item.status === "failed" || item.status === "cancelled" ? <p className="video-row-error"><AlertCircle size={14} />{errorLabel(item)}</p> : item.status === "noAudio" ? <p className="video-row-error"><AlertCircle size={14} />{t("extractAudioNoTrackExplanation")}</p> : <div className="extract-audio-metadata"><span><small>{t("format")}</small><strong>{item.format ?? "—"}</strong></span><span><small>{t("videoDuration")}</small><strong>{duration(item.durationSeconds)}</strong></span><span><small>{t("fileSize")}</small><strong>{formatBytes(item.size)}</strong></span><span><small>{t("audioCodec")}</small><strong>{item.audioTracks?.[item.selectedTrack]?.codec ?? "—"}</strong></span><span><small>{t("extractAudioBitrate")}</small><strong>{item.audioTracks?.[item.selectedTrack]?.bitrate ? `${Math.round(item.audioTracks[item.selectedTrack].bitrate! / 1000)} kbps` : "—"}</strong></span><span><small>{t("extractAudioSampleRate")}</small><strong>{item.audioTracks?.[item.selectedTrack]?.sampleRate ? `${(item.audioTracks[item.selectedTrack].sampleRate! / 1000).toFixed(1)} kHz` : "—"}</strong></span><span><small>{t("extractAudioChannels")}</small><strong>{item.audioTracks?.[item.selectedTrack]?.channels ?? "—"}</strong></span></div>}
          {!!item.audioTracks?.length && !["noAudio", "failed"].includes(item.status) && <label className="audio-track-select"><span>{t("extractAudioTrack")}</span><select disabled={isProcessing} value={item.selectedTrack} onChange={(event) => setItems((current) => current.map((entry) => entry.path === item.path ? { ...entry, selectedTrack: Number(event.target.value), plan: undefined } : entry))}>{item.audioTracks.map((track) => <option value={track.audioIndex} key={track.streamIndex}>{t("extractAudioTrackNumber", { number: track.audioIndex + 1 })} · {track.language?.toUpperCase() || t("extractAudioLanguageUnknown")} · {track.codec} · {track.channels ?? "—"} ch{track.audioIndex === 0 ? ` · ${t("extractAudioDefaultTrack")}` : ""}</option>)}</select></label>}
          {item.result && <div className="audio-result"><div className="convert-result-mode"><CheckCircle2 size={14} />{t(item.result.extractWithoutReencoding ? "extractAudioWithoutReencoding" : "extractAudioReencoded")}</div><div className="convert-result-grid"><span><small>{t("extractAudioSourceVideo")}</small><strong>{item.result.sourceName}</strong></span><span><small>{t("extractAudioFinalFormat")}</small><strong>{item.result.finalFormat}</strong></span><span><small>{t("extractAudioPlannedCodec")}</small><strong>{item.result.finalCodec}</strong></span><span><small>{t("extractAudioBitrate")}</small><strong>{item.result.finalBitrate ? `${Math.round(item.result.finalBitrate / 1000)} kbps` : "—"}</strong></span><span><small>{t("extractAudioSampleRate")}</small><strong>{item.result.sampleRate ? `${item.result.sampleRate} Hz` : "—"}</strong></span><span><small>{t("extractAudioChannels")}</small><strong>{item.result.channels ?? "—"}</strong></span><span><small>{t("extractAudioTrack")}</small><strong>#{item.result.selectedAudioTrack} · {item.result.selectedAudioLanguage?.toUpperCase() || t("extractAudioLanguageUnknown")}</strong></span><span><small>{t("convertFinalSize")}</small><strong>{formatBytes(item.result.finalSize)}</strong></span><span><small>{t("processingDuration")}</small><strong>{duration(item.result.processingDurationSeconds)}</strong></span></div><div className="video-result-location"><FolderOpen size={13} /><span title={item.result.outputPath}>{item.result.outputPath}</span></div><div className="video-result-actions"><button className="text-button" type="button" onClick={() => setPreviewResult(item.result!)}><Play size={13} />{t("extractAudioPreview")}</button><button className="text-button" type="button" onClick={() => void openPath(item.result!.outputPath)}><FileAudio2 size={13} />{t("extractAudioOpen")}</button><button className="text-button" type="button" onClick={() => void revealItemInDir(item.result!.outputPath)}><FolderOpen size={13} />{t("openFolder")}</button></div></div>}
        </div><button className="video-remove-button" disabled={isProcessing} type="button" onClick={() => remove(item.path)}><Trash2 size={15} /><span>{t("remove")}</span></button>
      </article>)}</div>}
    </section>

    <div className={`convert-video-settings-grid extract-audio-settings ${isProcessing ? "disabled" : ""}`}>
      <section className="compress-panel convert-video-format-panel"><div className="video-section-title"><Music2 size={18} /><div><h2>{t("extractAudioOutputFormat")}</h2><p>{t("extractAudioOutputHint")}</p></div></div><div className="convert-format-options">{outputFormats.map((format) => <button type="button" className={outputFormat === format ? "active" : ""} key={format} onClick={() => setOutputFormat(format)}>{format === "m4a" ? "M4A / AAC" : format === "ogg" ? "OGG / Opus" : format.toUpperCase()}</button>)}</div>
        <label className="convert-select"><span>{t("extractAudioQuality")}</span><select value={quality} disabled={copyAudio || !lossyFormat} onChange={(event) => setQuality(event.target.value as Quality)}>{(["low", "standard", "high", "maximum"] as Quality[]).map((value) => <option key={value} value={value}>{qualityLabel(value)}</option>)}</select></label>
        <label className="convert-select"><span>{t("extractAudioCustomBitrate")}</span><select value={bitrate ?? ""} disabled={copyAudio || !lossyFormat} onChange={(event) => setBitrate(event.target.value ? Number(event.target.value) : null)}><option value="">{t("extractAudioUseQuality")}</option>{bitrates.map((value) => <option value={value} key={value}>{value} kbps</option>)}</select></label>
      </section>

      <section className="compress-panel convert-video-options-panel"><div className="video-section-title"><Settings2 size={18} /><div><h2>{t("audioOptions")}</h2><p>{t("extractAudioOptionsHint")}</p></div></div>
        <label className="convert-select"><span>{t("extractAudioSampleRate")}</span><select value={sampleRate} onChange={(event) => setSampleRate(event.target.value)}><option value="original">{t("extractAudioOriginal")}</option><option value="44100">44.1 kHz</option><option value="48000">48 kHz</option><option value="custom">{t("extractAudioCustom")}</option></select></label>
        {sampleRate === "custom" && <label className="convert-custom-single"><span>{t("extractAudioCustomSampleRate")}</span><input type="number" min="8000" max="192000" step="1" value={customSampleRate} onChange={(event) => setCustomSampleRate(event.target.value)} /></label>}
        {!sampleRateValid && <p className="convert-validation"><AlertCircle size={13} />{t("extractAudioInvalidSampleRate")}</p>}
        <label className="convert-select"><span>{t("extractAudioChannels")}</span><select value={channels} onChange={(event) => setChannels(event.target.value)}><option value="original">{t("extractAudioOriginal")}</option><option value="1">{t("extractAudioMono")}</option><option value="2">{t("extractAudioStereo")}</option></select></label>
        <div className="convert-checks"><label><input type="checkbox" checked={preserveMetadata} onChange={(event) => setPreserveMetadata(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><strong>{t("preserveMetadata")}</strong></label><label className={!copyAvailable && !copyAudio ? "disabled-option" : ""}><input type="checkbox" checked={copyAudio} disabled={!copyAvailable && !copyAudio} onChange={(event) => setCopyAudio(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><strong>{t("extractAudioCopyStream")}</strong></label></div>
        {!copyAvailable && validItems.length > 0 && !copyAudio && <p className="convert-validation"><AlertCircle size={13} />{t("extractAudioCopyUnavailable")}</p>}
      </section>
    </div>

    {!!validItems.length && <section className="compress-panel convert-plan-panel"><div className="video-section-title"><ListMusic size={18} /><div><h2>{t("extractAudioPlanTitle")}</h2><p>{t("extractAudioPlanHint")}</p></div></div><div className="convert-plan-list">{validItems.map((item) => <article className={`convert-plan ${item.plan?.valid ? "valid" : "invalid"}`} key={item.path}><header><strong>{item.name}</strong>{item.plan?.valid ? <span><CheckCircle2 size={13} />{t(item.plan.extractWithoutReencoding ? "extractAudioWithoutReencoding" : "extractAudioReencoded")}</span> : <span><AlertCircle size={13} />{t(planReasons[item.plan?.reason ?? ""] ?? "extractAudioPlanUnavailable")}</span>}</header><div><span><small>{t("extractAudioOriginalCodec")}</small><strong>{item.audioTracks?.[item.selectedTrack]?.codec ?? "—"}</strong></span><span><small>{t("extractAudioFinalFormat")}</small><strong>{outputFormat === "ogg" ? "OGG / Opus" : outputFormat.toUpperCase()}</strong></span><span><small>{t("extractAudioPlannedCodec")}</small><strong>{item.plan?.plannedCodec ?? "—"}</strong></span><span><small>{t("extractAudioPlannedBitrate")}</small><strong>{item.plan?.plannedBitrate ? `${Math.round(item.plan.plannedBitrate / 1000)} kbps` : t("extractAudioLossless")}</strong></span><span><small>{t("extractAudioPlannedSampleRate")}</small><strong>{item.plan?.plannedSampleRate ? `${item.plan.plannedSampleRate} Hz` : "—"}</strong></span><span><small>{t("extractAudioPlannedChannels")}</small><strong>{item.plan?.plannedChannels ?? "—"}</strong></span></div></article>)}</div></section>}

    <section className="compress-panel video-destination-panel"><div><FolderOpen size={19} /><span><strong>{t("destinationFolder")}</strong><small title={destination ?? ""}>{destination ?? t("videoAutomaticDestination")}</small></span></div><button className="secondary-button" disabled={isProcessing} type="button" onClick={async () => { const selected = await open({ directory: true, multiple: false }); if (typeof selected === "string") setDestination(selected); }}>{t("chooseFolder")}</button></section>
    {notice && <p className="convert-interface-notice"><CheckCircle2 size={14} />{notice}</p>}
    {activeProgress && <section className="video-progress-panel convert-progress"><div><span>{t(activeProgress.phase === "copying" ? "extractAudioCopying" : "extractAudioExtracting")}</span><strong>{activeProgress.name}</strong><b>{activeProgress.percent.toFixed(1)}%</b></div><progress max="100" value={activeProgress.percent} /><small>{t("videoProcessingFile", { current: batchPosition.current, total: batchPosition.total })} · {duration(activeProgress.processedSeconds)} / {duration(activeProgress.durationSeconds)}</small></section>}
    {isProcessing ? <button className="run-button video-cancel-button" type="button" onClick={() => void cancelExtraction()}><Ban size={19} />{t("extractAudioCancel")}</button> : <button className="run-button extract-audio-button" type="button" disabled={!canExtract} onClick={() => void runExtraction()}><FileAudio2 size={19} />{t("extractAudioAction")}</button>}
    {previewResult && <div className="video-preview-backdrop" role="presentation" onMouseDown={(event) => { if (event.currentTarget === event.target) setPreviewResult(null); }}><section className="video-preview-modal audio-preview-modal" role="dialog" aria-modal="true" aria-label={t("extractAudioPreview")}><header><div><strong>{previewResult.outputName}</strong><small>{previewResult.finalCodec} · {previewResult.sampleRate ?? "—"} Hz</small></div><button className="icon-button" type="button" onClick={() => setPreviewResult(null)}><X size={17} /></button></header><audio src={convertFileSrc(previewResult.outputPath)} controls autoPlay /></section></div>}
  </div>;
}
