import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useSettings } from "../settings";
import { appendProcessingHistoryEntry, createImageProcessingHistoryEntry } from "../history";
import {
  AlertCircle, Check, CheckCircle2, ChevronRight, CircleGauge, FolderOpen,
  HardDrive, ImagePlus, Images, LoaderCircle, MapPin, ShieldCheck, Trash2, UploadCloud,
} from "lucide-react";
import { useI18n, type TranslationKey } from "../i18n";

type CompressionMode = "recommended" | "customQuality" | "targetFileSize";
type ItemStatus = "reading" | "ready" | "processing" | "completed" | "warning" | "failed";

interface ImageInfo {
  path: string;
  name: string;
  format: string;
  width: number;
  height: number;
  size: number;
}

interface CompressionResult {
  success: boolean;
  originalSize: number;
  newSize?: number;
  savedBytes?: number;
  reductionPercent?: number;
  outputPath?: string;
  warningCode?: string;
  errorCode?: string;
  errorMessage?: string;
}

interface ImageItem extends ImageInfo {
  status: ItemStatus;
  errorCode?: string;
  errorMessage?: string;
  result?: CompressionResult;
}

const errorTranslations: Record<string, TranslationKey> = {
  unsupported_format: "unsupportedFormat", invalid_image: "invalidImage",
  folder_inaccessible: "folderInaccessible", disk_full: "diskFull",
  read_error: "readError", write_error: "writeError", encode_error: "encodeError",
  invalid_target: "invalidTarget",
};

const warningTranslations: Record<string, TranslationKey> = {
  target_unreachable: "targetUnreachable",
  target_unreachable_png: "targetUnreachablePng",
  png_target_not_exact: "pngTargetNotExact",
};

function fileNameFromPath(path: string) {
  return path.split(/[\\/]/).pop() || path;
}

export function formatBytes(bytes: number) {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const unit = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** unit;
  return `${value >= 10 || unit === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
}

function commandError(error: unknown): { code: string; message?: string } {
  if (typeof error === "object" && error !== null) {
    const value = error as Record<string, unknown>;
    return { code: String(value.code ?? "unknown_error"), message: typeof value.message === "string" ? value.message : undefined };
  }
  try {
    const parsed = JSON.parse(String(error)) as { code?: string; message?: string };
    return { code: parsed.code ?? "unknown_error", message: parsed.message };
  } catch {
    return { code: "unknown_error", message: String(error) };
  }
}

export function CompressImagesPage() {
  const { t, language, setLanguage } = useI18n();
  const { settings } = useSettings();
  const [items, setItems] = useState<ImageItem[]>([]);
  const [destination, setDestination] = useState<string | null>(null);
  const [saveInNewFolder, setSaveInNewFolder] = useState(true);
  const [mode, setMode] = useState<CompressionMode>("recommended");
  const [quality, setQuality] = useState(82);
  const [targetValue, setTargetValue] = useState(500);
  const [targetUnit, setTargetUnit] = useState<"KB" | "MB">("KB");
  const [isProcessing, setIsProcessing] = useState(false);
  const [progress, setProgress] = useState(0);
  const [pageError, setPageError] = useState<string | null>(null);

  const translateError = useCallback((code?: string, fallback?: string) => {
    const key = code ? errorTranslations[code] : undefined;
    return key ? t(key) : fallback || t("unknownError");
  }, [t]);

  const addPaths = useCallback(async (paths: string[]) => {
    const uniquePaths = paths.filter((path, index) => paths.indexOf(path) === index && !items.some((item) => item.path === path));
    if (!uniquePaths.length) return;
    setPageError(null);
    setItems((current) => [...current, ...uniquePaths.map((path) => ({
      path, name: fileNameFromPath(path), format: "—", width: 0, height: 0, size: 0, status: "reading" as const,
    }))]);
    await Promise.all(uniquePaths.map(async (path) => {
      try {
        const info = await invoke<ImageInfo>("inspect_image", { path });
        setItems((current) => current.map((item) => item.path === path ? { ...info, status: "ready" } : item));
      } catch (error) {
        const parsed = commandError(error);
        setItems((current) => current.map((item) => item.path === path ? {
          ...item, status: "failed", errorCode: parsed.code, errorMessage: parsed.message,
        } : item));
      }
    }));
  }, [items]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "drop") void addPaths(event.payload.paths);
    }).then((cleanup) => { if (disposed) cleanup(); else unlisten = cleanup; });
    return () => { disposed = true; unlisten?.(); };
  }, [addPaths]);

  async function chooseImages() {
    const selected = await open({
      multiple: true,
      directory: false,
      filters: [{ name: "JPG, PNG, WebP", extensions: ["jpg", "jpeg", "png", "webp"] }],
    });
    if (selected) await addPaths(Array.isArray(selected) ? selected : [selected]);
  }

  async function chooseDestination() {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === "string") setDestination(selected);
  }

  async function runCompression() {
    const queue = items.filter((item) => item.width > 0 && item.status !== "processing");
    if (!queue.length || isProcessing) { setPageError(t("chooseImagesFirst")); return; }
    setIsProcessing(true);
    setPageError(null);
    setProgress(0);
    setItems((current) => current.map((item) => item.width > 0 ? { ...item, status: "ready", result: undefined, errorCode: undefined, errorMessage: undefined } : item));
    const targetBytes = Math.round(targetValue * (targetUnit === "MB" ? 1024 * 1024 : 1024));

    for (let index = 0; index < queue.length; index += 1) {
      const item = queue[index];
      setItems((current) => current.map((entry) => entry.path === item.path ? { ...entry, status: "processing" } : entry));
      const startTime = Date.now();
      let result: CompressionResult;
      try {
        result = await invoke<CompressionResult>("compress_image", {
          request: {
            path: item.path,
            destinationDir: destination,
            saveInNewFolder,
            mode,
            quality,
            targetBytes: mode === "targetFileSize" ? targetBytes : null,
          },
        });
      } catch (error) {
        const parsed = commandError(error);
        result = { success: false, originalSize: item.size, errorCode: parsed.code, errorMessage: parsed.message };
      }
      const durationMs = Date.now() - startTime;
      void appendProcessingHistoryEntry(createImageProcessingHistoryEntry({
        tool: "Compress Images",
        sourceFileName: item.name,
        resultPath: result.outputPath,
        success: result.success,
        originalSize: result.originalSize,
        finalSize: result.newSize,
        durationMs,
        errorMessage: result.errorMessage,
      }), settings.enableProcessingHistory);
      setItems((current) => current.map((entry) => entry.path === item.path ? {
        ...entry,
        status: result.success ? (result.warningCode ? "warning" : "completed") : "failed",
        result,
        errorCode: result.errorCode,
        errorMessage: result.errorMessage,
      } : entry));
      setProgress(Math.round(((index + 1) / queue.length) * 100));
    }
    setIsProcessing(false);
  }

  const finished = items.filter((item) => item.result);
  const summary = useMemo(() => {
    const successful = finished.filter((item) => item.result?.success);
    const before = successful.reduce((total, item) => total + item.size, 0);
    const after = successful.reduce((total, item) => total + (item.result?.newSize ?? 0), 0);
    return { successful: successful.length, failures: finished.length - successful.length, before, after, saved: Math.max(0, before - after) };
  }, [finished]);

  const statusLabel = (status: ItemStatus) => t(status === "reading" ? "reading" : status);

  return (
    <div className="page compression-page">
      <header className="page-header compression-header">
        <div><span className="eyebrow">{t("moduleLabel")}</span><h1>{t("title")}</h1><p>{t("subtitle")}</p></div>
        <div className="compress-header-actions">
          <label className="language-select" title={t("language")}>
            <span>{language.toUpperCase()}</span>
            <select value={language} onChange={(event) => setLanguage(event.target.value as "en" | "fr" | "ar")} disabled={isProcessing}>
              <option value="en">English</option><option value="fr">Français</option><option value="ar">العربية</option>
            </select>
          </label>
          <div className="header-status"><ShieldCheck size={17} /><span>{t("offline")}</span></div>
        </div>
      </header>

      <section className="compress-drop-zone" onClick={() => !isProcessing && void chooseImages()}>
        <div className="compress-drop-icon"><UploadCloud size={28} /></div>
        <div><h2>{t("dropTitle")}</h2><p>{t("dropSubtitle")}</p></div>
        <button className="primary-button" type="button" disabled={isProcessing} onClick={(event) => { event.stopPropagation(); void chooseImages(); }}><ImagePlus size={17} />{t("addImages")}</button>
      </section>

      <section className="compress-panel file-panel">
        <div className="compress-panel-header">
          <div><h2>{t("selectedImages")}</h2><span>{t("imageCount", { count: items.length })}</span></div>
          {!!items.length && <button className="text-button danger" type="button" onClick={() => { setItems([]); setProgress(0); }} disabled={isProcessing}><Trash2 size={14} />{t("clearAll")}</button>}
        </div>
        {!items.length ? (
          <div className="empty-image-list"><Images size={25} /><strong>{t("noImages")}</strong><p>{t("noImagesHint")}</p></div>
        ) : (
          <div className="image-list">
            <div className="image-list-head"><span>{t("file")}</span><span>{t("format")}</span><span>{t("dimensions")}</span><span>{t("originalSize")}</span><span>{t("status")}</span><span /></div>
            {items.map((item) => (
              <div className={`image-row ${item.status}`} key={item.path}>
                <div className="file-cell"><span className="file-type-icon">{item.format === "—" ? "?" : item.format.slice(0, 1)}</span><div><strong title={item.path}>{item.name}</strong>{item.result?.outputPath && <small title={item.result.outputPath}>{t("output")}: {fileNameFromPath(item.result.outputPath)}</small>}</div></div>
                <span className="format-pill">{item.format}</span>
                <span>{item.width ? `${item.width} × ${item.height}` : "—"}</span>
                <span>{item.size ? formatBytes(item.size) : "—"}</span>
                <div className={`status-cell ${item.status}`}>{item.status === "processing" || item.status === "reading" ? <LoaderCircle size={14} className="spin" /> : item.status === "completed" || item.status === "warning" ? <CheckCircle2 size={14} /> : item.status === "failed" ? <AlertCircle size={14} /> : <CircleGauge size={14} />}<span>{statusLabel(item.status)}</span></div>
                <button className="icon-button" type="button" title={t("remove")} disabled={isProcessing} onClick={() => setItems((current) => current.filter((entry) => entry.path !== item.path))}><Trash2 size={15} /></button>
                {(item.errorCode || item.result?.warningCode) && <div className={`row-message ${item.errorCode ? "error" : "warn"}`}><AlertCircle size={14} />{item.errorCode ? translateError(item.errorCode, item.errorMessage) : t(warningTranslations[item.result!.warningCode!] ?? "unknownError")}</div>}
                {item.result?.success && <div className="result-strip"><span><small>{t("result")}</small><strong>{formatBytes(item.result.newSize ?? 0)}</strong></span><span><small>{t("saved")}</small><strong>{formatBytes(item.result.savedBytes ?? 0)} · {(item.result.reductionPercent ?? 0).toFixed(1)}%</strong></span><span className="output-path" title={item.result.outputPath}><MapPin size={13} />{item.result.outputPath}</span><button className="text-button" type="button" onClick={() => item.result?.outputPath && void revealItemInDir(item.result.outputPath)}><FolderOpen size={14} />{t("openFolder")}</button></div>}
              </div>
            ))}
          </div>
        )}
      </section>

      <div className="compress-settings-grid">
        <section className="compress-panel destination-panel">
          <div className="setting-title"><HardDrive size={18} /><div><h2>{t("destination")}</h2><p>{destination ?? t("automaticFolder")}</p></div></div>
          <button className="secondary-button" type="button" onClick={() => void chooseDestination()} disabled={isProcessing}><FolderOpen size={15} />{t("chooseFolder")}</button>
          <label className="check-option"><input type="checkbox" checked={saveInNewFolder} onChange={(event) => setSaveInNewFolder(event.target.checked)} disabled={isProcessing} /><span className="custom-check"><Check size={13} /></span><span><strong>{t("saveNewFolder")}</strong><small>{t("saveNewFolderHint")}</small></span></label>
        </section>

        <section className="compress-panel mode-panel">
          <div className="setting-title"><CircleGauge size={18} /><div><h2>{t("compressionMode")}</h2></div></div>
          <div className="mode-options">
            {(["recommended", "customQuality", "targetFileSize"] as CompressionMode[]).map((value) => (
              <button className={`mode-option ${mode === value ? "active" : ""}`} type="button" key={value} onClick={() => setMode(value)} disabled={isProcessing}><span className="radio-dot" /><span><strong>{t(value)}</strong><small>{t(`${value}Hint` as TranslationKey)}</small></span><ChevronRight size={15} /></button>
            ))}
          </div>
          {mode === "customQuality" && <div className="quality-control"><div><strong>{t("quality")}</strong><output>{quality}</output></div><input type="range" min="10" max="100" value={quality} onChange={(event) => setQuality(Number(event.target.value))} /><div className="range-labels"><span>{t("smaller")}</span><span>{t("higher")}</span></div></div>}
          {mode === "targetFileSize" && <div className="target-control"><strong>{t("targetSize")}</strong><div className="target-presets">{[[100,"KB"],[500,"KB"],[1,"MB"],[2,"MB"]].map(([value, unit]) => <button type="button" key={`${value}${unit}`} onClick={() => { setTargetValue(Number(value)); setTargetUnit(unit as "KB" | "MB"); }}>{value} {unit}</button>)}</div><div className="target-input"><input type="number" min="0.01" step="0.01" value={targetValue} onChange={(event) => setTargetValue(Number(event.target.value))} /><select value={targetUnit} onChange={(event) => setTargetUnit(event.target.value as "KB" | "MB")}><option>KB</option><option>MB</option></select></div><div className="png-notice"><AlertCircle size={15} /><span>{t("pngNotice")}</span></div></div>}
        </section>
      </div>

      {isProcessing || progress > 0 ? <div className="global-progress"><div><span>{t("progress")}</span><strong>{progress}%</strong></div><progress max="100" value={progress} /></div> : null}
      {pageError && <div className="page-error"><AlertCircle size={16} />{pageError}</div>}

      {!!finished.length && !isProcessing && <section className="summary-panel"><div className="summary-title"><CheckCircle2 size={20} /><h2>{t("summary")}</h2></div><div className="summary-grid"><span><small>{t("succeeded")}</small><strong>{summary.successful}</strong></span><span><small>{t("failures")}</small><strong>{summary.failures}</strong></span><span><small>{t("totalBefore")}</small><strong>{formatBytes(summary.before)}</strong></span><span><small>{t("totalAfter")}</small><strong>{formatBytes(summary.after)}</strong></span><span className="saved-summary"><small>{t("totalSaved")}</small><strong>{formatBytes(summary.saved)}</strong></span></div></section>}

      <button className="compress-primary-action" type="button" onClick={() => void runCompression()} disabled={isProcessing || !items.some((item) => item.width > 0)}>{isProcessing ? <LoaderCircle className="spin" size={19} /> : <ImagePlus size={19} />}<span>{isProcessing ? t("compressing") : t("compressImages")}</span></button>
    </div>
  );
}
