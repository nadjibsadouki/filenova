import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import {
  AlertCircle,
  Check,
  CheckCircle2,
  FileAudio2,
  FolderOpen,
  ListMusic,
  Music2,
  Settings2,
  Trash2,
  UploadCloud,
} from "lucide-react";
import { useI18n, type TranslationKey } from "../i18n";

type OutputFormat = "mp3" | "wav" | "m4a" | "flac" | "ogg";
type Quality = "low" | "standard" | "high" | "maximum";
type ItemStatus = "analyzing" | "ready" | "noAudio" | "failed";

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
    && validItems.every((item) => copyCompatible(outputFormat, item.audioCodec));
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
  const canExtract = plansValid && sampleRateValid && outputFormats.includes(outputFormat);

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
    }))]);
    for (const path of selected) {
      try {
        const info = await invoke<AudioSourceInfo>("inspect_audio_source", { path });
        setItems((current) => current.map((item) => item.path === path
          ? { ...info, status: info.audioCodec ? "ready" : "noAudio" }
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
    Promise.all(validItems.map(async (item) => ({
      path: item.path,
      plan: await invoke<AudioPlan>("plan_audio_extraction", {
        request: {
          inputAudioCodec: item.audioCodec,
          inputAudioBitrate: item.audioBitrate,
          inputSampleRate: item.sampleRate,
          inputChannels: item.channels,
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
  }, [bitrate, channels, copyAudio, lossyFormat, outputFormat, quality, requestedSampleRate, sampleRateValid, validItems.map((item) => item.path).join("|")]);

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

  return <div className="page extract-audio-page">
    <header className="page-header compression-header video-header">
      <div><span className="eyebrow">{t("extractAudioModuleLabel")}</span><h1>{t("extractAudioTitle")}</h1><p>{t("extractAudioSubtitle")}</p></div>
      <div className="compress-header-actions"><label className="language-select"><span>{language.toUpperCase()}</span><select value={language} onChange={(event) => setLanguage(event.target.value as "en" | "fr" | "ar")}><option value="en">English</option><option value="fr">Français</option><option value="ar">العربية</option></select></label><div className="header-status"><Music2 size={17} /><span>FFprobe</span></div></div>
    </header>

    <section className="video-drop-zone extract-audio-drop" onClick={() => void chooseVideos()}>
      <div className="video-drop-icon"><UploadCloud size={34} /></div><div><h2>{t("extractAudioDropTitle")}</h2><p>{t("extractAudioDropHint")}</p></div>
      <button className="primary-button" type="button" onClick={(event) => { event.stopPropagation(); void chooseVideos(); }}><FileAudio2 size={17} />{t("addVideos")}</button>
    </section>

    <section className="compress-panel extract-audio-files">
      <div className="compress-panel-header"><div><h2>{t("selectedVideos")}</h2><span>{t("videoCount", { count: items.length })}</span></div>{!!items.length && <button className="text-button danger" type="button" onClick={() => setItems([])}><Trash2 size={14} />{t("clearAll")}</button>}</div>
      {!items.length ? <div className="video-empty-state"><ListMusic size={30} /><strong>{t("extractAudioEmpty")}</strong><p>{t("extractAudioEmptyHint")}</p></div> : <div className="extract-audio-list">{items.map((item) => <article className={`extract-audio-row ${item.status}`} key={item.path}>
        <div className="extract-audio-icon"><FileAudio2 size={22} /></div><div className="extract-audio-file"><div className="video-file-title"><div><strong>{item.name}</strong><small title={item.path}>{item.path}</small></div><div className={`video-status ${item.status}`}><span>{t(item.status === "analyzing" ? "analyzingVideo" : item.status === "ready" ? "videoReady" : item.status === "noAudio" ? "extractAudioNoTrack" : "videoFailed")}</span></div></div>
          {item.status === "failed" ? <p className="video-row-error"><AlertCircle size={14} />{t("videoInspectionFailed")}</p> : item.status === "noAudio" ? <p className="video-row-error"><AlertCircle size={14} />{t("extractAudioNoTrackExplanation")}</p> : <div className="extract-audio-metadata"><span><small>{t("format")}</small><strong>{item.format ?? "—"}</strong></span><span><small>{t("videoDuration")}</small><strong>{duration(item.durationSeconds)}</strong></span><span><small>{t("fileSize")}</small><strong>{formatBytes(item.size)}</strong></span><span><small>{t("audioCodec")}</small><strong>{item.audioCodec ?? "—"}</strong></span><span><small>{t("extractAudioBitrate")}</small><strong>{item.audioBitrate ? `${Math.round(item.audioBitrate / 1000)} kbps` : "—"}</strong></span><span><small>{t("extractAudioSampleRate")}</small><strong>{item.sampleRate ? `${(item.sampleRate / 1000).toFixed(1)} kHz` : "—"}</strong></span><span><small>{t("extractAudioChannels")}</small><strong>{item.channels ?? "—"}</strong></span></div>}
        </div><button className="video-remove-button" type="button" onClick={() => remove(item.path)}><Trash2 size={15} /><span>{t("remove")}</span></button>
      </article>)}</div>}
    </section>

    <div className="convert-video-settings-grid extract-audio-settings">
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

    {!!validItems.length && <section className="compress-panel convert-plan-panel"><div className="video-section-title"><ListMusic size={18} /><div><h2>{t("extractAudioPlanTitle")}</h2><p>{t("extractAudioPlanHint")}</p></div></div><div className="convert-plan-list">{validItems.map((item) => <article className={`convert-plan ${item.plan?.valid ? "valid" : "invalid"}`} key={item.path}><header><strong>{item.name}</strong>{item.plan?.valid ? <span><CheckCircle2 size={13} />{t(item.plan.extractWithoutReencoding ? "extractAudioWithoutReencoding" : "extractAudioReencoded")}</span> : <span><AlertCircle size={13} />{t(planReasons[item.plan?.reason ?? ""] ?? "extractAudioPlanUnavailable")}</span>}</header><div><span><small>{t("extractAudioOriginalCodec")}</small><strong>{item.audioCodec}</strong></span><span><small>{t("extractAudioFinalFormat")}</small><strong>{outputFormat === "ogg" ? "OGG / Opus" : outputFormat.toUpperCase()}</strong></span><span><small>{t("extractAudioPlannedCodec")}</small><strong>{item.plan?.plannedCodec ?? "—"}</strong></span><span><small>{t("extractAudioPlannedBitrate")}</small><strong>{item.plan?.plannedBitrate ? `${Math.round(item.plan.plannedBitrate / 1000)} kbps` : t("extractAudioLossless")}</strong></span><span><small>{t("extractAudioPlannedSampleRate")}</small><strong>{item.plan?.plannedSampleRate ? `${item.plan.plannedSampleRate} Hz` : "—"}</strong></span><span><small>{t("extractAudioPlannedChannels")}</small><strong>{item.plan?.plannedChannels ?? "—"}</strong></span></div></article>)}</div></section>}

    <section className="compress-panel video-destination-panel"><div><FolderOpen size={19} /><span><strong>{t("destinationFolder")}</strong><small title={destination ?? ""}>{destination ?? t("videoAutomaticDestination")}</small></span></div><button className="secondary-button" type="button" onClick={async () => { const selected = await open({ directory: true, multiple: false }); if (typeof selected === "string") setDestination(selected); }}>{t("chooseFolder")}</button></section>
    {notice && <p className="convert-interface-notice"><CheckCircle2 size={14} />{notice}</p>}
    <button className="run-button extract-audio-button" type="button" disabled={!canExtract} onClick={() => setNotice(t("extractAudioNotImplemented"))}><FileAudio2 size={19} />{t("extractAudioAction")}</button>
  </div>;
}
