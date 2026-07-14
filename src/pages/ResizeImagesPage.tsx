import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  AlertTriangle,
  Check,
  CheckCircle2,
  CircleGauge,
  Crop,
  FileImage,
  FolderOpen,
  ImagePlus,
  Images,
  LoaderCircle,
  Maximize2,
  Palette,
  Percent,
  Scaling,
  ShieldCheck,
  SlidersHorizontal,
  Smartphone,
  Trash2,
  UploadCloud,
  X,
} from "lucide-react";
import { useI18n, type TranslationKey } from "../i18n";

type ResizeStatus = "reading" | "ready" | "processing" | "completed" | "failed" | "cancelled";
type ResizeMode = "exact" | "percentage" | "maximum" | "social";
type ExactAxis = "width" | "height";
type AdjustmentMethod = "fit" | "fill" | "stretch";
type FitBackground = "white" | "black" | "transparent" | "custom";
type ResizeOutputFormat = "jpg" | "png" | "webp";

const percentagePresets = [25, 50, 75, 150, 200];
const socialPresets = [
  { id: "instagram-square-post", platform: "Instagram", labelKey: "presetSquarePost", width: 1080, height: 1080 },
  { id: "instagram-portrait-post", platform: "Instagram", labelKey: "presetPortraitPost", width: 1080, height: 1350 },
  { id: "instagram-story-reel", platform: "Instagram", labelKey: "presetStoryReel", width: 1080, height: 1920 },
  { id: "youtube-thumbnail", platform: "YouTube", labelKey: "presetThumbnail", width: 1280, height: 720 },
  { id: "youtube-channel-banner", platform: "YouTube", labelKey: "presetChannelBanner", width: 2560, height: 1440 },
  { id: "tiktok-vertical-cover", platform: "TikTok", labelKey: "presetVerticalCover", width: 1080, height: 1920 },
  { id: "facebook-post", platform: "Facebook", labelKey: "presetPost", width: 1200, height: 630 },
  { id: "facebook-cover", platform: "Facebook", labelKey: "presetCover", width: 1640, height: 624 },
  { id: "linkedin-post", platform: "LinkedIn", labelKey: "presetPost", width: 1200, height: 627 },
  { id: "linkedin-profile-banner", platform: "LinkedIn", labelKey: "presetProfileBanner", width: 1584, height: 396 },
  { id: "x-post", platform: "X / Twitter", labelKey: "presetPost", width: 1600, height: 900 },
  { id: "x-header", platform: "X / Twitter", labelKey: "presetHeader", width: 1500, height: 500 },
] as const;

const socialPlatforms = ["Instagram", "YouTube", "TikTok", "Facebook", "LinkedIn", "X / Twitter"] as const;

const resizeErrorKeys: Record<string, TranslationKey> = {
  invalid_image: "resizeCorruptedFile",
  unsupported_conversion_format: "resizeUnsupportedFormat",
  unsupported_resize_format: "resizeUnsupportedFormat",
  invalid_dimensions: "resizeInvalidDimensions",
  invalid_percentage: "resizeInvalidPercentage",
  dimensions_too_large: "resizeDimensionsTooLarge",
  folder_inaccessible: "folderInaccessible",
  disk_full: "diskFull",
  read_error: "readError",
  write_error: "writeError",
  encode_error: "encodeError",
  invalid_background_color: "invalidBackgroundColor",
  transparency_incompatible_jpg: "resizeTransparencyJpg",
  processing_error: "resizeProcessingError",
};

interface ResizableImageInfo {
  path: string;
  name: string;
  format: string;
  width: number;
  height: number;
  size: number;
}

interface ResizeItem extends ResizableImageInfo {
  status: ResizeStatus;
  result?: ResizeResult;
  errorCode?: string;
  errorMessage?: string;
}

interface ResizeResult {
  success: boolean;
  originalWidth?: number;
  originalHeight?: number;
  finalWidth?: number;
  finalHeight?: number;
  originalSize: number;
  newSize?: number;
  finalFormat?: string;
  outputPath?: string;
  errorCode?: string;
  errorMessage?: string;
}

function parseCommandError(error: unknown): { code: string; message?: string } {
  if (typeof error === "object" && error !== null) {
    const value = error as Record<string, unknown>;
    return { code: String(value.code ?? "processing_error"), message: typeof value.message === "string" ? value.message : undefined };
  }
  try {
    const parsed = JSON.parse(String(error)) as { code?: string; message?: string };
    return { code: parsed.code ?? "processing_error", message: parsed.message };
  } catch {
    return { code: "processing_error", message: String(error) };
  }
}

function fileNameFromPath(path: string) {
  return path.split(/[\\/]/).pop() || path;
}

function formatBytes(value: number) {
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const unit = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  const amount = value / 1024 ** unit;
  return `${amount >= 10 || unit === 0 ? amount.toFixed(0) : amount.toFixed(1)} ${units[unit]}`;
}

export function ResizeImagesPage() {
  const { t, language, setLanguage } = useI18n();
  const [items, setItems] = useState<ResizeItem[]>([]);
  const [destination, setDestination] = useState<string | null>(null);
  const [mode, setMode] = useState<ResizeMode>("exact");
  const [exactWidth, setExactWidth] = useState(1920);
  const [exactHeight, setExactHeight] = useState(1080);
  const [maintainRatio, setMaintainRatio] = useState(true);
  const [lastExactAxis, setLastExactAxis] = useState<ExactAxis>("width");
  const [percentage, setPercentage] = useState(50);
  const [maximumWidth, setMaximumWidth] = useState(1920);
  const [maximumHeight, setMaximumHeight] = useState(1080);
  const [socialPresetId, setSocialPresetId] = useState<string>(socialPresets[0].id);
  const [customSocialWidth, setCustomSocialWidth] = useState(1080);
  const [customSocialHeight, setCustomSocialHeight] = useState(1080);
  const [adjustmentMethod, setAdjustmentMethod] = useState<AdjustmentMethod>("fit");
  const [fitBackground, setFitBackground] = useState<FitBackground>("white");
  const [customBackground, setCustomBackground] = useState("#6c58dc");
  const [doNotEnlarge, setDoNotEnlarge] = useState(true);
  const [preserveFormat, setPreserveFormat] = useState(true);
  const [outputFormat, setOutputFormat] = useState<ResizeOutputFormat>("jpg");
  const [outputQuality, setOutputQuality] = useState(90);
  const [removeResizeMetadata, setRemoveResizeMetadata] = useState(false);
  const [isProcessing, setIsProcessing] = useState(false);
  const [isCancelling, setIsCancelling] = useState(false);
  const [progress, setProgress] = useState(0);
  const [pageError, setPageError] = useState<string | null>(null);
  const cancelRequested = useRef(false);

  const referenceImage = items.find((item) => item.status === "ready" && item.width > 0);

  function updateExactWidth(value: number) {
    const width = Math.max(1, Math.round(value || 1));
    setExactWidth(width);
    setLastExactAxis("width");
    if (maintainRatio && referenceImage) {
      setExactHeight(Math.max(1, Math.round(width * referenceImage.height / referenceImage.width)));
    }
  }

  function updateExactHeight(value: number) {
    const height = Math.max(1, Math.round(value || 1));
    setExactHeight(height);
    setLastExactAxis("height");
    if (maintainRatio && referenceImage) {
      setExactWidth(Math.max(1, Math.round(height * referenceImage.width / referenceImage.height)));
    }
  }

  const estimatedDimensions = useMemo(() => {
    const estimates = new Map<string, { width: number; height: number }>();
    const selectedSocialPreset = socialPresets.find((preset) => preset.id === socialPresetId);

    for (const item of items) {
      if (!item.width || !item.height) continue;
      let width = item.width;
      let height = item.height;

      if (mode === "exact") {
        if (!maintainRatio) {
          width = exactWidth;
          height = exactHeight;
        } else if (lastExactAxis === "width") {
          width = exactWidth;
          height = Math.max(1, Math.round(width * item.height / item.width));
        } else {
          height = exactHeight;
          width = Math.max(1, Math.round(height * item.width / item.height));
        }
      } else if (mode === "percentage") {
        const scale = Math.max(1, percentage) / 100;
        width = Math.max(1, Math.round(item.width * scale));
        height = Math.max(1, Math.round(item.height * scale));
      } else if (mode === "maximum") {
        const scale = Math.min(1, maximumWidth / item.width, maximumHeight / item.height);
        width = Math.max(1, Math.round(item.width * scale));
        height = Math.max(1, Math.round(item.height * scale));
      } else {
        width = selectedSocialPreset?.width ?? customSocialWidth;
        height = selectedSocialPreset?.height ?? customSocialHeight;
      }

      if (doNotEnlarge && item.width <= width && item.height <= height) {
        width = item.width;
        height = item.height;
      }

      estimates.set(item.path, { width, height });
    }
    return estimates;
  }, [customSocialHeight, customSocialWidth, doNotEnlarge, exactHeight, exactWidth, items, lastExactAxis, maintainRatio, maximumHeight, maximumWidth, mode, percentage, socialPresetId]);

  const addPaths = useCallback(async (paths: string[]) => {
    const uniquePaths = paths.filter(
      (path, index) => paths.indexOf(path) === index && !items.some((item) => item.path === path),
    );
    if (!uniquePaths.length) return;

    setItems((current) => [...current, ...uniquePaths.map((path) => ({
      path,
      name: fileNameFromPath(path),
      format: "—",
      width: 0,
      height: 0,
      size: 0,
      status: "reading" as const,
    }))]);

    await Promise.all(uniquePaths.map(async (path) => {
      try {
        const info = await invoke<ResizableImageInfo>("inspect_convertible_image", { path });
        setItems((current) => current.map((item) => (
          item.path === path ? { ...info, status: "ready" } : item
        )));
      } catch (error) {
        const parsed = parseCommandError(error);
        setItems((current) => current.map((item) => (
          item.path === path ? { ...item, status: "failed", errorCode: parsed.code, errorMessage: parsed.message } : item
        )));
      }
    }));
  }, [items]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "drop") void addPaths(event.payload.paths);
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [addPaths]);

  async function selectImages() {
    const selected = await open({
      multiple: true,
      directory: false,
      filters: [{
        name: "JPG, JPEG, PNG, WebP, BMP, TIFF",
        extensions: ["jpg", "jpeg", "png", "webp", "bmp", "tif", "tiff"],
      }],
    });
    if (selected) await addPaths(Array.isArray(selected) ? selected : [selected]);
  }

  async function selectDestination() {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === "string") setDestination(selected);
  }

  async function resizeAll() {
    const queue = items.filter((item) => item.status !== "failed" && item.width > 0 && estimatedDimensions.has(item.path));
    if (!queue.length || isProcessing) {
      setPageError(t("resizeImagesFirst"));
      return;
    }

    setIsProcessing(true);
    setIsCancelling(false);
    setProgress(0);
    setPageError(null);
    cancelRequested.current = false;
    setItems((current) => current.map((item) => item.width > 0 ? {
      ...item,
      status: "ready",
      result: undefined,
      errorCode: undefined,
      errorMessage: undefined,
    } : item));

    for (let index = 0; index < queue.length; index += 1) {
      if (cancelRequested.current) {
        const remaining = new Set(queue.slice(index).map((item) => item.path));
        setItems((current) => current.map((entry) => remaining.has(entry.path) ? { ...entry, status: "cancelled" } : entry));
        break;
      }
      const item = queue[index];
      const estimate = estimatedDimensions.get(item.path)!;
      setItems((current) => current.map((entry) => entry.path === item.path ? { ...entry, status: "processing" } : entry));

      let result: ResizeResult;
      try {
        const backgroundColor = fitBackground === "transparent"
          ? "transparent"
          : fitBackground === "black"
            ? "#111111"
            : fitBackground === "custom"
              ? customBackground
              : "#ffffff";
        result = await invoke<ResizeResult>("resize_image", { request: {
          path: item.path,
          destinationDir: destination,
          mode: mode === "social" ? "exact" : mode,
          width: mode === "maximum" ? maximumWidth : estimate.width,
          height: mode === "maximum" ? maximumHeight : estimate.height,
          percentage,
          adjustmentMethod,
          doNotEnlarge,
          preserveOriginalFormat: preserveFormat,
          outputFormat,
          quality: outputQuality,
          backgroundColor,
          removeMetadata: removeResizeMetadata,
        }});
      } catch (error) {
        const parsed = parseCommandError(error);
        result = {
          success: false,
          originalSize: item.size,
          errorCode: parsed.code,
          errorMessage: parsed.message,
        };
      }

      setItems((current) => current.map((entry) => entry.path === item.path ? {
        ...entry,
        status: result.success ? "completed" : "failed",
        result,
        errorCode: result.errorCode,
        errorMessage: result.errorMessage,
      } : entry));
      setProgress(Math.round(((index + 1) / queue.length) * 100));
    }
    setIsProcessing(false);
    setIsCancelling(false);
  }

  function cancelSafely() {
    cancelRequested.current = true;
    setIsCancelling(true);
  }

  function statusLabel(status: ResizeStatus) {
    if (status === "processing") return t("resizing");
    return t(status);
  }

  function errorLabel(item: ResizeItem) {
    return item.errorCode ? t(resizeErrorKeys[item.errorCode] ?? "resizeProcessingError") : item.errorMessage ?? t("resizeProcessingError");
  }

  const selectedSocialPreset = socialPresets.find((preset) => preset.id === socialPresetId);
  const chosenDimensions = mode === "percentage"
    ? `${percentage}%`
    : mode === "maximum"
      ? `${maximumWidth} × ${maximumHeight} px (${t("maximumDimensions")})`
      : mode === "social"
        ? `${selectedSocialPreset?.width ?? customSocialWidth} × ${selectedSocialPreset?.height ?? customSocialHeight} px`
        : maintainRatio
          ? `${exactWidth} × ${exactHeight} px (${t("maintainAspectRatio")})`
          : `${exactWidth} × ${exactHeight} px`;
  const batchSummary = {
    total: items.length,
    success: items.filter((item) => item.status === "completed").length,
    failed: items.filter((item) => item.status === "failed").length,
  };
  const hasBatchResults = items.some((item) => item.result || item.status === "cancelled");

  return (
    <div className="page compression-page resize-page">
      <header className="page-header compression-header">
        <div>
          <span className="eyebrow">{t("resizeModuleLabel")}</span>
          <h1>{t("resizeTitle")}</h1>
          <p>{t("resizeSubtitle")}</p>
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
          <div className="header-status"><ShieldCheck size={17} /><span>{t("offline")}</span></div>
        </div>
      </header>

      <section className="resize-drop-zone" onClick={() => !isProcessing && void selectImages()}>
        <div className="compress-drop-icon"><UploadCloud size={28} /></div>
        <div><h2>{t("resizeDropTitle")}</h2><p>{t("resizeDropSubtitle")}</p></div>
        <button className="primary-button" type="button" disabled={isProcessing} onClick={(event) => { event.stopPropagation(); void selectImages(); }}>
          <ImagePlus size={17} />{t("addImages")}
        </button>
      </section>

      <section className="compress-panel file-panel">
        <div className="compress-panel-header">
          <div><h2>{t("selectedImages")}</h2><span>{t("imageCount", { count: items.length })}</span></div>
          {!!items.length && (
            <button className="text-button danger" type="button" disabled={isProcessing} onClick={() => { setItems([]); setProgress(0); }}>
              <Trash2 size={14} />{t("clearAll")}
            </button>
          )}
        </div>

        {!items.length ? (
          <div className="empty-image-list"><Images size={25} /><strong>{t("noImages")}</strong><p>{t("noImagesHint")}</p></div>
        ) : (
          <div className="image-list resize-list">
            <div className="resize-list-head">
              <span>{t("file")}</span><span>{t("format")}</span><span>{t("dimensions")}</span>
              <span>{t("estimatedDimensions")}</span><span>{t("fileSize")}</span><span>{t("status")}</span><span />
            </div>
            {items.map((item) => {
              const estimate = estimatedDimensions.get(item.path);
              return (
              <div className={`resize-row ${item.status}`} key={item.path}>
                <div className="file-cell">
                  <span className="image-thumbnail">
                    <FileImage size={16} />
                    {item.status !== "failed" && (
                      <img src={convertFileSrc(item.path)} alt="" onError={(event) => { event.currentTarget.style.display = "none"; }} />
                    )}
                  </span>
                  <div><strong title={item.path}>{item.name}</strong><small>{item.format === "—" ? t("imagePreview") : `${item.width} × ${item.height} px`}</small></div>
                </div>
                <span className="format-pill">{item.format}</span>
                <span>{item.width ? `${item.width} × ${item.height}` : "—"}</span>
                <strong className="estimated-size">{estimate ? `${estimate.width} × ${estimate.height}` : "—"}</strong>
                <span>{item.size ? formatBytes(item.size) : "—"}</span>
                <div className={`status-cell ${item.status}`}>
                  {item.status === "reading" || item.status === "processing" ? <LoaderCircle className="spin" size={14} /> : item.status === "completed" ? <CheckCircle2 size={14} /> : item.status === "failed" ? <AlertTriangle size={14} /> : item.status === "cancelled" ? <X size={14} /> : <CircleGauge size={14} />}
                  <span>{statusLabel(item.status)}</span>
                </div>
                <button className="icon-button" type="button" disabled={isProcessing} title={t("remove")} onClick={() => setItems((current) => current.filter((entry) => entry.path !== item.path))}>
                  <Trash2 size={15} />
                </button>
                {(item.errorCode || item.errorMessage) && <div className="row-message error"><AlertTriangle size={14} />{errorLabel(item)}</div>}
                {item.result?.success && (
                  <div className="resize-result-strip">
                    <span><small>{t("originalDimensions")}</small><strong>{item.result.originalWidth} × {item.result.originalHeight}</strong></span>
                    <span><small>{t("finalDimensions")}</small><strong>{item.result.finalWidth} × {item.result.finalHeight}</strong></span>
                    <span><small>{t("originalSize")}</small><strong>{formatBytes(item.result.originalSize)}</strong></span>
                    <span><small>{t("finalSize")}</small><strong>{formatBytes(item.result.newSize ?? 0)}</strong></span>
                    <span><small>{t("finalFormat")}</small><strong>{item.result.finalFormat}</strong></span>
                    <span><small>{t("status")}</small><strong>{statusLabel(item.status)}</strong></span>
                    <span className="output-path" title={item.result.outputPath}><small>{t("fileLocation")}</small>{item.result.outputPath}</span>
                    <button className="text-button" type="button" onClick={() => item.result?.outputPath && void revealItemInDir(item.result.outputPath)}><FolderOpen size={14} />{t("openFolder")}</button>
                  </div>
                )}
              </div>
              );
            })}
          </div>
        )}
      </section>

      <section className="compress-panel resize-controls-panel">
        <div className="setting-title"><Scaling size={18} /><div><h2>{t("resizeMode")}</h2><p>{t("resizeModeHint")}</p></div></div>
        <div className="resize-mode-tabs">
          {([
            ["exact", Maximize2, "exactDimensions"],
            ["percentage", Percent, "percentage"],
            ["maximum", Scaling, "maximumDimensions"],
            ["social", Smartphone, "socialMediaPresets"],
          ] as const).map(([value, Icon, label]) => (
            <button className={mode === value ? "active" : ""} type="button" key={value} onClick={() => setMode(value)}>
              <Icon size={16} /><span>{t(label)}</span>{mode === value && <Check size={13} />}
            </button>
          ))}
        </div>

        <div className="resize-mode-content">
          {mode === "exact" && (
            <div className="resize-control-group">
              <div className="dimension-inputs">
                <label><span>{t("width")}</span><div><input type="number" min="1" value={exactWidth} onChange={(event) => updateExactWidth(Number(event.target.value))} /><b>px</b></div></label>
                <span className="dimension-link">×</span>
                <label><span>{t("height")}</span><div><input type="number" min="1" value={exactHeight} onChange={(event) => updateExactHeight(Number(event.target.value))} /><b>px</b></div></label>
              </div>
              <label className="check-option standalone resize-ratio-option">
                <input type="checkbox" checked={maintainRatio} onChange={(event) => setMaintainRatio(event.target.checked)} />
                <span className="custom-check"><Check size={13} /></span><span><strong>{t("maintainAspectRatio")}</strong><small>{t("ratioPerImageHint")}</small></span>
              </label>
            </div>
          )}

          {mode === "percentage" && (
            <div className="resize-control-group">
              <div className="percentage-presets">
                {percentagePresets.map((value) => <button className={percentage === value ? "active" : ""} type="button" key={value} onClick={() => setPercentage(value)}>{value}%</button>)}
              </div>
              <label className="custom-percentage"><span>{t("customPercentage")}</span><div><input type="number" min="1" value={percentage} onChange={(event) => setPercentage(Math.max(1, Math.round(Number(event.target.value) || 1)))} /><b>%</b></div></label>
            </div>
          )}

          {mode === "maximum" && (
            <div className="resize-control-group">
              <div className="dimension-inputs">
                <label><span>{t("maximumWidth")}</span><div><input type="number" min="1" value={maximumWidth} onChange={(event) => setMaximumWidth(Math.max(1, Math.round(Number(event.target.value) || 1)))} /><b>px</b></div></label>
                <span className="dimension-link">×</span>
                <label><span>{t("maximumHeight")}</span><div><input type="number" min="1" value={maximumHeight} onChange={(event) => setMaximumHeight(Math.max(1, Math.round(Number(event.target.value) || 1)))} /><b>px</b></div></label>
              </div>
              <div className="ratio-safety-note"><ShieldCheck size={16} /><span><strong>{t("proportionsPreserved")}</strong><small>{t("neverDistortHint")}</small></span></div>
            </div>
          )}

          {mode === "social" && (
            <div className="resize-control-group social-control-group">
              <div className="social-preset-list">
                {socialPlatforms.map((platform) => (
                  <div className="social-preset-group" key={platform}>
                    <h3>{platform}</h3>
                    <div>
                      {socialPresets.filter((preset) => preset.platform === platform).map((preset) => (
                        <button className={socialPresetId === preset.id ? "active" : ""} type="button" key={preset.id} onClick={() => setSocialPresetId(preset.id)}>
                          <span><strong>{t(preset.labelKey)}</strong><small>{preset.width} × {preset.height} px</small></span>{socialPresetId === preset.id && <Check size={14} />}
                        </button>
                      ))}
                    </div>
                  </div>
                ))}
                <div className="social-preset-group custom-preset-group">
                  <h3>{t("customPreset")}</h3>
                  <button className={socialPresetId === "custom" ? "active" : ""} type="button" onClick={() => setSocialPresetId("custom")}>
                    <span><strong>{t("customDimensions")}</strong><small>{customSocialWidth} × {customSocialHeight} px</small></span>{socialPresetId === "custom" && <Check size={14} />}
                  </button>
                  {socialPresetId === "custom" && (
                    <div className="custom-social-inputs">
                      <label><span>{t("width")}</span><div><input type="number" min="1" value={customSocialWidth} onChange={(event) => setCustomSocialWidth(Math.max(1, Math.round(Number(event.target.value) || 1)))} /><b>px</b></div></label>
                      <span>×</span>
                      <label><span>{t("height")}</span><div><input type="number" min="1" value={customSocialHeight} onChange={(event) => setCustomSocialHeight(Math.max(1, Math.round(Number(event.target.value) || 1)))} /><b>px</b></div></label>
                    </div>
                  )}
                </div>
              </div>
            </div>
          )}
        </div>
      </section>

      <div className="resize-options-grid">
        <section className="compress-panel adjustment-panel">
          <div className="setting-title"><Crop size={18} /><div><h2>{t("adjustmentMethod")}</h2><p>{t("adjustmentMethodHint")}</p></div></div>
          <div className="adjustment-methods">
            {([
              ["fit", "fitMethod", "fitHint"],
              ["fill", "fillMethod", "fillHint"],
              ["stretch", "stretchMethod", "stretchHint"],
            ] as const).map(([value, label, hint]) => (
              <button className={adjustmentMethod === value ? "active" : ""} type="button" key={value} onClick={() => setAdjustmentMethod(value)}>
                <span className="radio-dot" /><span><strong>{t(label)}</strong><small>{t(hint)}</small></span>{adjustmentMethod === value && <Check size={14} />}
              </button>
            ))}
          </div>

          {adjustmentMethod === "fit" && (
            <div className="fit-background-control">
              <div className="fit-background-title"><Palette size={15} /><strong>{t("fitBackground")}</strong></div>
              <div className="background-options">
                {([
                  ["white", "#ffffff", "backgroundWhite"],
                  ["black", "#111111", "backgroundBlack"],
                  ["transparent", "transparent", "backgroundTransparent"],
                  ["custom", customBackground, "backgroundCustom"],
                ] as const).map(([value, color, label]) => {
                  const transparencyUnavailable = value === "transparent" && !preserveFormat && outputFormat === "jpg";
                  return (
                    <button className={fitBackground === value ? "active" : ""} type="button" key={value} disabled={transparencyUnavailable} title={transparencyUnavailable ? t("transparentUnavailableJpg") : undefined} onClick={() => setFitBackground(value)}>
                      <span className={`background-swatch ${value}`} style={value === "custom" ? { background: color } : value !== "transparent" ? { background: color } : undefined} />
                      <span>{t(label)}</span>{fitBackground === value && <Check size={12} />}
                    </button>
                  );
                })}
              </div>
              {fitBackground === "custom" && <label className="custom-background-picker"><span>{t("customColor")}</span><input type="color" value={customBackground} onChange={(event) => setCustomBackground(event.target.value)} /><code>{customBackground.toUpperCase()}</code></label>}
              {fitBackground === "transparent" && preserveFormat && <p className="transparency-format-note">{t("transparentFormatHint")}</p>}
            </div>
          )}

          {adjustmentMethod === "stretch" && <div className="stretch-warning"><AlertTriangle size={17} /><span><strong>{t("stretchWarning")}</strong><small>{t("stretchWarningHint")}</small></span></div>}
        </section>

        <section className="compress-panel resize-output-panel">
          <div className="setting-title"><SlidersHorizontal size={18} /><div><h2>{t("resizeOutputOptions")}</h2><p>{t("resizeOutputOptionsHint")}</p></div></div>
          <div className="resize-output-checks">
            <label className="check-option standalone">
              <input type="checkbox" checked={doNotEnlarge} onChange={(event) => setDoNotEnlarge(event.target.checked)} />
              <span className="custom-check"><Check size={13} /></span><span><strong>{t("doNotEnlarge")}</strong><small>{t("doNotEnlargeHint")}</small></span>
            </label>
            <label className="check-option standalone">
              <input type="checkbox" checked={preserveFormat} onChange={(event) => {
                const preserve = event.target.checked;
                setPreserveFormat(preserve);
                if (!preserve && outputFormat === "jpg" && fitBackground === "transparent") setFitBackground("white");
              }} />
              <span className="custom-check"><Check size={13} /></span><span><strong>{t("preserveOriginalFormat")}</strong><small>{t("preserveOriginalFormatHint")}</small></span>
            </label>
          </div>

          <div className={`resize-format-control ${preserveFormat ? "disabled" : ""}`}>
            <div><strong>{t("resizeOutputFormat")}</strong><small>{preserveFormat ? t("disablePreserveToChoose") : t("resizeOutputFormatHint")}</small></div>
            <div className="resize-format-buttons">
              {(["jpg", "png", "webp"] as ResizeOutputFormat[]).map((format) => (
                <button className={outputFormat === format ? "active" : ""} type="button" key={format} disabled={preserveFormat} onClick={() => {
                  setOutputFormat(format);
                  if (format === "jpg" && fitBackground === "transparent") setFitBackground("white");
                }}>{format.toUpperCase()}{outputFormat === format && <Check size={12} />}</button>
              ))}
            </div>
          </div>

          {!preserveFormat && (outputFormat === "jpg" || outputFormat === "webp") && (
            <div className="quality-control resize-quality-control">
              <div><strong>{t("jpegWebpQuality")}</strong><output>{outputQuality}</output></div>
              <input type="range" min="10" max="100" value={outputQuality} onChange={(event) => setOutputQuality(Number(event.target.value))} />
              <div className="range-labels"><span>10</span><span>100</span></div>
            </div>
          )}

          <label className="check-option standalone remove-resize-metadata">
            <input type="checkbox" checked={removeResizeMetadata} onChange={(event) => setRemoveResizeMetadata(event.target.checked)} />
            <span className="custom-check"><Check size={13} /></span><span><strong>{t("removeMetadata")}</strong><small>{t("removeMetadataResizeHint")}</small></span>
          </label>
        </section>
      </div>

      <section className="compress-panel resize-destination-panel">
        <div className="setting-title"><FolderOpen size={18} /><div><h2>{t("destination")}</h2><p title={destination ?? undefined}>{destination ?? t("automaticFolder")}</p></div></div>
        <button className="secondary-button" type="button" disabled={isProcessing} onClick={() => void selectDestination()}>
          <FolderOpen size={15} />{t("chooseFolder")}
        </button>
      </section>

      {(isProcessing || progress > 0) && <div className="global-progress"><div><span>{isCancelling ? t("cancelling") : t("progress")}</span><strong>{progress}%</strong></div><progress max="100" value={progress} /></div>}
      {pageError && <div className="page-error"><AlertTriangle size={16} />{pageError}</div>}

      {hasBatchResults && !isProcessing && (
        <section className="summary-panel">
          <div className="summary-title"><CheckCircle2 size={20} /><h2>{t("resizeBatchSummary")}</h2></div>
          <div className="resize-summary-grid">
            <span><small>{t("totalFiles")}</small><strong>{batchSummary.total}</strong></span>
            <span><small>{t("succeeded")}</small><strong>{batchSummary.success}</strong></span>
            <span><small>{t("failures")}</small><strong>{batchSummary.failed}</strong></span>
            <span><small>{t("chosenDimensions")}</small><strong>{chosenDimensions}</strong></span>
            <span><small>{t("destination")}</small><strong title={destination ?? undefined}>{destination ?? t("automaticFolder")}</strong></span>
          </div>
        </section>
      )}

      <div className="convert-actions">
        {isProcessing && <button className="cancel-action" type="button" disabled={isCancelling} onClick={cancelSafely}><X size={17} />{isCancelling ? t("cancelling") : t("cancel")}</button>}
        <button className="compress-primary-action convert-main-action" type="button" disabled={isProcessing || !items.some((item) => item.width > 0)} onClick={() => void resizeAll()}>
          {isProcessing ? <LoaderCircle className="spin" size={19} /> : <Scaling size={19} />}<span>{isProcessing ? t("resizing") : t("resizeImages")}</span>
        </button>
      </div>
    </div>
  );
}
