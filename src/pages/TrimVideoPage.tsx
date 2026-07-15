import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import {
  AlertCircle,
  Check,
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
} from "lucide-react";
import { useI18n, type TranslationKey } from "../i18n";

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

const extensions = ["mp4", "mov", "mkv", "avi", "webm", "m4v"];

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

function copyCompatible(format: OutputFormat, codec: string) {
  const value = codec.toUpperCase().replace(/-/g, "");
  if (format === "webm") return ["VP8", "VP9", "AV1"].includes(value);
  if (format === "mp4") return ["H264", "HEVC", "AV1", "MPEG4"].includes(value);
  if (format === "mov") return ["H264", "HEVC", "MPEG4", "PRORES"].includes(value);
  return value !== "UNKNOWN";
}

function defaultOutputName(name: string) {
  const stem = name.replace(/\.[^.]+$/, "") || "video";
  return `${stem}-trimmed`;
}

export function TrimVideoPage() {
  const { t, language, setLanguage } = useI18n();
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
  const [destination, setDestination] = useState<string | null>(null);
  const [outputName, setOutputName] = useState("");
  const player = useRef<HTMLVideoElement>(null);
  const previewing = useRef(false);

  const loadVideo = useCallback(async (paths: string[]) => {
    const path = paths.find((value) => extensions.includes(value.split(".").pop()?.toLowerCase() ?? ""));
    if (!path) return;
    setStatus("analyzing");
    setError(null);
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
    let disposed = false;
    let cleanup: (() => void) | undefined;
    getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "drop") void loadVideo(event.payload.paths.slice(0, 1));
    }).then((unlisten) => {
      if (disposed) unlisten(); else cleanup = unlisten;
    });
    return () => { disposed = true; cleanup?.(); };
  }, [loadVideo]);

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
  const noReencoding = !!video && mode === "fast" && (preserveFormat || copyCompatible(outputFormat, video.videoCodec));
  const startPercent = video?.durationSeconds ? Math.max(0, Math.min(100, start / video.durationSeconds * 100)) : 0;
  const endPercent = video?.durationSeconds ? Math.max(0, Math.min(100, end / video.durationSeconds * 100)) : 100;

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

      <div className="convert-video-settings-grid trim-settings-grid"><section className="compress-panel trim-mode-panel"><div className="video-section-title"><Scissors size={18} /><div><h2>{t("trimPrecision")}</h2><p>{t("trimPrecisionHint")}</p></div></div><div className="trim-mode-options">{(["fast", "precise"] as CutMode[]).map((value) => <button type="button" className={mode === value ? "active" : ""} key={value} onClick={() => setMode(value)}><strong>{t(value === "fast" ? "trimFastCut" : "trimPreciseCut")}</strong><small>{t(value === "fast" ? "trimFastCutDescription" : "trimPreciseCutDescription")}</small></button>)}</div></section>
        <section className="compress-panel convert-video-options-panel"><div className="video-section-title"><Settings2 size={18} /><div><h2>{t("videoOptions")}</h2><p>{t("trimOptionsHint")}</p></div></div><div className="convert-checks"><label><input type="checkbox" checked={preserveFormat} onChange={(event) => setPreserveFormat(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><strong>{t("trimPreserveFormat")}</strong></label><label><input type="checkbox" checked={preserveMetadata} onChange={(event) => setPreserveMetadata(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><strong>{t("preserveMetadata")}</strong></label><label><input type="checkbox" checked={removeAudio} onChange={(event) => setRemoveAudio(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><strong>{t("removeAudio")}</strong></label></div><label className="convert-select"><span>{t("convertOutputFormat")}</span><select value={outputFormat} disabled={preserveFormat} onChange={(event) => setOutputFormat(event.target.value as OutputFormat)}><option value="mp4">MP4</option><option value="mov">MOV</option><option value="mkv">MKV</option><option value="webm">WebM</option></select></label></section></div>

      <section className="compress-panel trim-plan-panel"><div className="video-section-title"><Clapperboard size={18} /><div><h2>{t("trimPlanTitle")}</h2><p>{t("trimPlanHint")}</p></div></div><div className="convert-plan valid"><div><span><small>{t("trimStartTime")}</small><strong>{formatTrimTime(start)}</strong></span><span><small>{t("trimEndTime")}</small><strong>{formatTrimTime(end)}</strong></span><span><small>{t("trimSelectedDuration")}</small><strong>{formatTrimTime(selectedDuration)}</strong></span><span><small>{t("convertOriginalFormat")}</small><strong>{video.format}</strong></span><span><small>{t("convertFinalFormat")}</small><strong>{finalFormat}</strong></span><span><small>{t("trimMethod")}</small><strong>{t(mode === "fast" ? "trimFastCut" : "trimPreciseCut")}</strong></span></div><p className={noReencoding ? "trim-plan-copy" : "trim-plan-encode"}>{t(noReencoding ? "trimNoReencodingPlanned" : "trimReencodingRequired")}</p></div></section>

      <section className="compress-panel video-destination-panel"><div><FolderOpen size={19} /><span><strong>{t("destinationFolder")}</strong><small title={destination ?? ""}>{destination ?? t("videoAutomaticDestination")}</small></span></div><button className="secondary-button" type="button" onClick={async () => { const selected = await open({ directory: true, multiple: false }); if (typeof selected === "string") setDestination(selected); }}>{t("chooseFolder")}</button></section><label className="compress-panel trim-output-name"><span>{t("trimOutputName")}</span><input value={outputName} onChange={(event) => setOutputName(event.target.value)} /></label>
      <button className="run-button trim-video-button" type="button" disabled title={selectionValid ? t("trimNotImplemented") : selectionError ? t(selectionError) : undefined}><Scissors size={19} />{t("trimVideoAction")}</button>
    </>}
  </div>;
}
