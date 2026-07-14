import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  AlertTriangle, Check, CheckCircle2, CircleGauge, FileImage, FolderOpen, ImagePlus,
  Images, LoaderCircle, MapPin, RefreshCw, ShieldCheck, Square, Trash2, UploadCloud, X,
} from "lucide-react";
import { useI18n, type TranslationKey } from "../i18n";

type OutputFormat = "jpg" | "png" | "webp" | "bmp" | "tiff";
type ConvertStatus = "reading" | "ready" | "processing" | "completed" | "failed" | "cancelled";

interface ConvertibleInfo {
  path: string; name: string; format: string; width: number; height: number; size: number; hasTransparency: boolean;
}

interface ConvertResult {
  success: boolean; sourceFormat?: string; targetFormat: string; originalSize: number; newSize?: number;
  outputPath?: string; errorCode?: string; errorMessage?: string;
}

interface ConvertItem extends ConvertibleInfo {
  status: ConvertStatus; progress: number; errorCode?: string; errorMessage?: string; result?: ConvertResult;
}

const outputFormats: { value: OutputFormat; label: string }[] = [
  { value: "jpg", label: "JPG" }, { value: "png", label: "PNG" }, { value: "webp", label: "WebP" },
  { value: "bmp", label: "BMP" }, { value: "tiff", label: "TIFF" },
];

const errorKeys: Record<string, TranslationKey> = {
  invalid_image: "invalidImage", unsupported_conversion_format: "unsupportedConversionFormat",
  folder_inaccessible: "folderInaccessible", read_error: "readError", write_error: "writeError",
  disk_full: "diskFull", encode_error: "encodeError",
  transparency_confirmation_required: "transparencyRequired", invalid_background_color: "invalidBackgroundColor",
};

function nameFromPath(path: string) { return path.split(/[\\/]/).pop() || path; }

function bytes(value: number) {
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const unit = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  const amount = value / 1024 ** unit;
  return `${amount >= 10 || unit === 0 ? amount.toFixed(0) : amount.toFixed(1)} ${units[unit]}`;
}

function parseError(error: unknown) {
  if (typeof error === "object" && error !== null) {
    const data = error as Record<string, unknown>;
    return { code: String(data.code ?? "unknown_error"), message: typeof data.message === "string" ? data.message : undefined };
  }
  try { return JSON.parse(String(error)) as { code: string; message?: string }; }
  catch { return { code: "unknown_error", message: String(error) }; }
}

export function ConvertImagesPage() {
  const { t, language, setLanguage } = useI18n();
  const [items, setItems] = useState<ConvertItem[]>([]);
  const [targetFormat, setTargetFormat] = useState<OutputFormat>("webp");
  const [destination, setDestination] = useState<string | null>(null);
  const [quality, setQuality] = useState(85);
  const [optimizePng, setOptimizePng] = useState(true);
  const [removeMetadata, setRemoveMetadata] = useState(true);
  const [backgroundColor, setBackgroundColor] = useState("#ffffff");
  const [confirmTransparency, setConfirmTransparency] = useState(false);
  const [isProcessing, setIsProcessing] = useState(false);
  const [isCancelling, setIsCancelling] = useState(false);
  const [globalProgress, setGlobalProgress] = useState(0);
  const [pageError, setPageError] = useState<string | null>(null);
  const cancelRequested = useRef(false);

  const addPaths = useCallback(async (paths: string[]) => {
    const unique = paths.filter((path, index) => paths.indexOf(path) === index && !items.some((item) => item.path === path));
    if (!unique.length) return;
    setPageError(null);
    setItems((current) => [...current, ...unique.map((path) => ({
      path, name: nameFromPath(path), format: "—", width: 0, height: 0, size: 0,
      hasTransparency: false, status: "reading" as const, progress: 0,
    }))]);
    await Promise.all(unique.map(async (path) => {
      try {
        const info = await invoke<ConvertibleInfo>("inspect_convertible_image", { path });
        setItems((current) => current.map((item) => item.path === path ? { ...info, status: "ready", progress: 0 } : item));
      } catch (error) {
        const parsed = parseError(error);
        setItems((current) => current.map((item) => item.path === path ? { ...item, status: "failed", progress: 0, errorCode: parsed.code, errorMessage: parsed.message } : item));
      }
    }));
  }, [items]);

  useEffect(() => {
    let cleanup: (() => void) | undefined;
    let disposed = false;
    getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "drop") void addPaths(event.payload.paths);
    }).then((unlisten) => { if (disposed) unlisten(); else cleanup = unlisten; });
    return () => { disposed = true; cleanup?.(); };
  }, [addPaths]);

  async function selectImages() {
    const selected = await open({ multiple: true, directory: false, filters: [{ name: "Images", extensions: ["jpg", "jpeg", "png", "webp", "bmp", "tif", "tiff"] }] });
    if (selected) await addPaths(Array.isArray(selected) ? selected : [selected]);
  }

  async function selectDestination() {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === "string") setDestination(selected);
  }

  const containsTransparency = items.some((item) => item.width > 0 && item.hasTransparency);
  const needsTransparencyApproval = targetFormat === "jpg" && containsTransparency;

  async function convertAll() {
    const queue = items.filter((item) => item.width > 0);
    if (!queue.length || isProcessing) { setPageError(t("conversionReadyHint")); return; }
    if (needsTransparencyApproval && !confirmTransparency) { setPageError(t("transparencyRequired")); return; }
    setPageError(null); setIsProcessing(true); setIsCancelling(false); setGlobalProgress(0); cancelRequested.current = false;
    setItems((current) => current.map((item) => item.width > 0 ? { ...item, status: "ready", progress: 0, result: undefined, errorCode: undefined, errorMessage: undefined } : item));

    for (let index = 0; index < queue.length; index += 1) {
      if (cancelRequested.current) {
        const remaining = new Set(queue.slice(index).map((item) => item.path));
        setItems((current) => current.map((item) => remaining.has(item.path) ? { ...item, status: "cancelled", progress: 0 } : item));
        break;
      }
      const item = queue[index];
      setItems((current) => current.map((entry) => entry.path === item.path ? { ...entry, status: "processing", progress: 35 } : entry));
      let result: ConvertResult;
      try {
        result = await invoke<ConvertResult>("convert_image", { request: {
          path: item.path, destinationDir: destination, targetFormat, quality, optimizePng,
          backgroundColor, confirmTransparency: !item.hasTransparency || confirmTransparency,
          keepDimensions: true, removeMetadata,
        }});
      } catch (error) {
        const parsed = parseError(error);
        result = { success: false, targetFormat: targetFormat.toUpperCase(), originalSize: item.size, errorCode: parsed.code, errorMessage: parsed.message };
      }
      setItems((current) => current.map((entry) => entry.path === item.path ? {
        ...entry, status: result.success ? "completed" : "failed", progress: 100, result,
        errorCode: result.errorCode, errorMessage: result.errorMessage,
      } : entry));
      setGlobalProgress(Math.round(((index + 1) / queue.length) * 100));
    }
    setIsProcessing(false); setIsCancelling(false);
  }

  function cancelSafely() { cancelRequested.current = true; setIsCancelling(true); }

  const results = items.filter((item) => item.result);
  const summary = useMemo(() => ({
    success: results.filter((item) => item.result?.success).length,
    failed: results.filter((item) => !item.result?.success).length,
  }), [results]);

  const statusText = (status: ConvertStatus) => t(status);
  const messageFor = (item: ConvertItem) => item.errorCode ? t(errorKeys[item.errorCode] ?? "unknownError") : item.errorMessage;

  return (
    <div className="page conversion-page">
      <header className="page-header compression-header">
        <div><span className="eyebrow">{t("convertModuleLabel")}</span><h1>{t("convertTitle")}</h1><p>{t("convertSubtitle")}</p></div>
        <div className="compress-header-actions">
          <label className="language-select"><span>{language.toUpperCase()}</span><select value={language} onChange={(event) => setLanguage(event.target.value as "en" | "fr" | "ar")} disabled={isProcessing}><option value="en">English</option><option value="fr">Français</option><option value="ar">العربية</option></select></label>
          <div className="header-status"><ShieldCheck size={17} /><span>{t("offline")}</span></div>
        </div>
      </header>

      <section className="convert-drop-zone" onClick={() => !isProcessing && void selectImages()}>
        <div className="compress-drop-icon"><UploadCloud size={28} /></div><div><h2>{t("convertDropTitle")}</h2><p>{t("convertDropSubtitle")}</p></div>
        <button className="primary-button" type="button" disabled={isProcessing} onClick={(event) => { event.stopPropagation(); void selectImages(); }}><ImagePlus size={17} />{t("addImages")}</button>
      </section>

      <section className="compress-panel file-panel">
        <div className="compress-panel-header"><div><h2>{t("selectedImages")}</h2><span>{t("imageCount", { count: items.length })}</span></div>{!!items.length && <button className="text-button danger" type="button" disabled={isProcessing} onClick={() => { setItems([]); setGlobalProgress(0); }}><Trash2 size={14} />{t("clearAll")}</button>}</div>
        {!items.length ? <div className="empty-image-list"><Images size={25} /><strong>{t("noImages")}</strong><p>{t("noImagesHint")}</p></div> :
          <div className="image-list convert-list">
            <div className="convert-list-head"><span>{t("file")}</span><span>{t("currentFormat")}</span><span>{t("dimensions")}</span><span>{t("fileSize")}</span><span>{t("status")}</span><span /></div>
            {items.map((item) => <div className={`convert-row ${item.status}`} key={item.path}>
              <div className="file-cell"><span className="file-type-icon"><FileImage size={15} /></span><div><strong title={item.path}>{item.name}</strong>{item.hasTransparency && <small>Alpha / Transparency</small>}</div></div>
              <span className="format-pill">{item.format}</span><span>{item.width ? `${item.width} × ${item.height}` : "—"}</span><span>{item.size ? bytes(item.size) : "—"}</span>
              <div className={`status-cell ${item.status}`}>{item.status === "processing" || item.status === "reading" ? <LoaderCircle className="spin" size={14} /> : item.status === "completed" ? <CheckCircle2 size={14} /> : item.status === "failed" ? <AlertTriangle size={14} /> : item.status === "cancelled" ? <X size={14} /> : <CircleGauge size={14} />}<span>{statusText(item.status)}</span></div>
              <button className="icon-button" type="button" disabled={isProcessing} title={t("remove")} onClick={() => setItems((current) => current.filter((entry) => entry.path !== item.path))}><Trash2 size={15} /></button>
              {(item.status === "processing" || item.status === "completed") && <progress className="row-progress" max="100" value={item.progress} />}
              {item.errorCode && <div className="row-message error"><AlertTriangle size={14} />{messageFor(item)}</div>}
              {item.result?.success && <div className="convert-result-strip"><span><small>{t("sourceFormat")}</small><strong>{item.result.sourceFormat}</strong></span><span><small>{t("finalFormat")}</small><strong>{item.result.targetFormat}</strong></span><span><small>{t("originalSize")}</small><strong>{bytes(item.result.originalSize)}</strong></span><span><small>{t("finalSize")}</small><strong>{bytes(item.result.newSize ?? 0)}</strong></span><span className="output-path" title={item.result.outputPath}><MapPin size={13} />{item.result.outputPath}</span><button className="text-button" type="button" onClick={() => item.result?.outputPath && void revealItemInDir(item.result.outputPath)}><FolderOpen size={14} />{t("openFolder")}</button></div>}
            </div>)}
          </div>}
      </section>

      <div className="convert-settings-grid">
        <section className="compress-panel output-format-panel">
          <div className="setting-title"><RefreshCw size={18} /><div><h2>{t("outputFormat")}</h2><p>{t("outputFormatHint")}</p></div></div>
          <div className="format-selector">{outputFormats.map((format) => <button className={targetFormat === format.value ? "active" : ""} type="button" key={format.value} disabled={isProcessing} onClick={() => { setTargetFormat(format.value); setConfirmTransparency(false); }}><span>{format.label}</span><Check size={13} /></button>)}</div>
          {(targetFormat === "jpg" || targetFormat === "webp") && <div className="quality-control convert-quality"><div><strong>{t("jpegWebpQuality")}</strong><output>{quality}</output></div><input type="range" min="10" max="100" value={quality} disabled={isProcessing} onChange={(event) => setQuality(Number(event.target.value))} /><div className="range-labels"><span>{t("smaller")}</span><span>{t("higher")}</span></div></div>}
          {targetFormat === "png" && <label className="check-option standalone"><input type="checkbox" checked={optimizePng} disabled={isProcessing} onChange={(event) => setOptimizePng(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><span><strong>{t("optimizePng")}</strong><small>{t("optimizePngHint")}</small></span></label>}
        </section>

        <section className="compress-panel conversion-options-panel">
          <div className="setting-title"><Square size={18} /><div><h2>{t("conversionOptions")}</h2></div></div>
          <div className="conversion-checks">
            <label className="check-option standalone locked"><input type="checkbox" checked readOnly /><span className="custom-check"><Check size={13} /></span><span><strong>{t("keepDimensions")}</strong><small>{t("safetyLocked")}</small></span></label>
            <label className="check-option standalone"><input type="checkbox" checked={removeMetadata} disabled={isProcessing} onChange={(event) => setRemoveMetadata(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><span><strong>{t("removeMetadata")}</strong></span></label>
            <label className="check-option standalone locked"><input type="checkbox" checked readOnly /><span className="custom-check"><Check size={13} /></span><span><strong>{t("keepOriginals")}</strong><small>{t("safetyLocked")}</small></span></label>
          </div>
          <div className="destination-choice"><div><strong>{t("destination")}</strong><small title={destination ?? undefined}>{destination ?? t("automaticFolder")}</small></div><button className="secondary-button" type="button" disabled={isProcessing} onClick={() => void selectDestination()}><FolderOpen size={15} />{t("chooseFolder")}</button></div>
        </section>
      </div>

      {needsTransparencyApproval && <section className="transparency-warning"><AlertTriangle size={21} /><div><h2>{t("transparencyWarning")}</h2><p>{t("transparencyExplanation")}</p><div className="background-choice"><label>{t("backgroundColor")}<input type="color" value={backgroundColor} disabled={isProcessing} onChange={(event) => setBackgroundColor(event.target.value)} /></label><code>{backgroundColor.toUpperCase()}</code></div><label className="confirm-transparency"><input type="checkbox" checked={confirmTransparency} disabled={isProcessing} onChange={(event) => setConfirmTransparency(event.target.checked)} /><span className="custom-check"><Check size={13} /></span><span>{t("confirmTransparency")}</span></label></div></section>}

      {(isProcessing || globalProgress > 0) && <div className="global-progress"><div><span>{isCancelling ? t("cancelling") : t("progress")}</span><strong>{globalProgress}%</strong></div><progress max="100" value={globalProgress} /></div>}
      {pageError && <div className="page-error"><AlertTriangle size={16} />{pageError}</div>}
      {!!results.length && !isProcessing && <section className="summary-panel"><div className="summary-title"><CheckCircle2 size={20} /><h2>{t("conversionSummary")}</h2></div><div className="conversion-summary"><span><small>{t("succeeded")}</small><strong>{summary.success}</strong></span><span><small>{t("failures")}</small><strong>{summary.failed}</strong></span><span><small>{t("outputFormat")}</small><strong>{targetFormat.toUpperCase()}</strong></span></div></section>}

      <div className="convert-actions">{isProcessing && <button className="cancel-action" type="button" disabled={isCancelling} onClick={cancelSafely}><X size={17} />{isCancelling ? t("cancelling") : t("cancel")}</button>}<button className="compress-primary-action convert-main-action" type="button" disabled={isProcessing || !items.some((item) => item.width > 0)} onClick={() => void convertAll()}>{isProcessing ? <LoaderCircle className="spin" size={19} /> : <RefreshCw size={19} />}<span>{isProcessing ? t("converting") : t("convertImages")}</span></button></div>
    </div>
  );
}
