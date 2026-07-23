import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  appendProcessingHistoryEntry,
  appendProcessingHistoryEntries,
  createPdfProcessingHistoryEntry,
  type ProcessingHistoryEntry,
} from "../history";
import { useSettings } from "../settings";
import { ArrowDown, ArrowLeft, ArrowRight, ArrowUp, CheckCircle2, FileImage, FilePlus2, Files, FolderOpen, GripVertical, ImagePlus, Images, Layers3, ListOrdered, LoaderCircle, MapPin, RotateCcw, RotateCw, Save, Scissors, ShieldCheck, Trash2, Undo2, UploadCloud, X } from "lucide-react";
import { useI18n, type TranslationKey } from "../i18n";

type Tab = "merge" | "split" | "organize" | "imagesToPdf" | "pdfToImages";
type ItemStatus = "reading" | "ready" | "failed";
interface PdfInfo { path: string; name: string; size: number; pages: number; }
interface PdfItem extends PdfInfo { status: ItemStatus; errorCode?: string; errorMessage?: string; }
interface MergeResult { success: boolean; pdfCount: number; totalPages: number; newSize?: number; outputPath?: string; errorCode?: string; errorMessage?: string; }
interface SplitOutput { path: string; name: string; pages: number; size: number; }
interface SplitResult { success: boolean; files: SplitOutput[]; totalPages: number; destinationDir?: string; errorCode?: string; errorMessage?: string; }
interface SplitProgress { completed: number; total: number; percent: number; currentFile: string; }
interface OrganizeResult { success: boolean; initialPages: number; finalPages: number; outputPath?: string; newSize?: number; errorCode?: string; errorMessage?: string; }
interface PdfImageInfo { path: string; name: string; format: string; width: number; height: number; size: number; }
interface PdfImageItem extends PdfImageInfo { status: ItemStatus; errorCode?: string; errorMessage?: string; }
interface ImagesPdfResult { success: boolean; imageCount: number; pageCount: number; pageSizeUsed?: string; outputPath?: string; newSize?: number; errorCode?: string; errorMessage?: string; }
interface ImagesPdfProgress { completed: number; total: number; percent: number; currentFile: string; }
interface PdfImagesProgress { completed: number; total: number; percent: number; currentPage: number; currentFile: string; }
interface PdfImagesOutput { path: string; name: string; page: number; width: number; height: number; size: number; format: string; }
interface PdfImagesFailure { page: number; code: string; message: string; }
interface PdfImagesResult { success: boolean; files: PdfImagesOutput[]; failures: PdfImagesFailure[]; totalPages: number; failedCount: number; format: string; dpi: number; totalSize: number; destinationDir?: string; errorCode?: string; errorMessage?: string; }
interface PdfImageOpenValidation { path: string; size: number; format: string; }

const tabs: { id: Tab; key: TranslationKey }[] = [
  { id: "merge", key: "mergePdfs" }, { id: "split", key: "splitPdf" },
  { id: "organize", key: "organizePages" }, { id: "imagesToPdf", key: "imagesToPdf" },
  { id: "pdfToImages", key: "pdfToImages" },
];
const errorKeys: Record<string, TranslationKey> = {
  corrupted_pdf: "pdfCorrupted", not_pdf: "pdfNotPdf", password_protected_pdf: "pdfPasswordProtected",
  destination_inaccessible: "pdfDestinationInaccessible", read_error: "pdfReadError", write_error: "pdfWriteError", encode_error: "encodeError",
  insufficient_space: "pdfInsufficientSpace", not_enough_pdfs: "pdfNeedTwo", unknown_error: "unknownError",
  split_invalid_pages: "splitInvalidPages", split_page_zero: "splitPageZero", split_range_reversed: "splitRangeReversed",
  split_page_out_of_range: "splitPageOutOfRange", split_invalid_ranges: "splitInvalidRanges",
  split_invalid_n: "splitInvalidN", split_invalid_mode: "splitInvalidMode", split_already_running: "splitAlreadyRunning",
  organize_all_pages_deleted: "organizeNeedsPage", organize_page_out_of_range: "organizeInvalidPage",
  organize_already_running: "organizeAlreadyRunning",
  invalid_image: "invalidImage", unsupported_conversion_format: "unsupportedConversionFormat",
  images_pdf_no_images: "imagesPdfNeedImages", images_pdf_invalid_quality: "imagesPdfInvalidQuality",
  images_pdf_invalid_color: "imagesPdfInvalidColor", images_pdf_invalid_margin: "imagesPdfInvalidMargin",
  images_pdf_invalid_page_size: "imagesPdfInvalidPageSize", images_pdf_invalid_orientation: "imagesPdfInvalidOrientation",
  images_pdf_invalid_dimensions: "imagesPdfInvalidDimensions",
  images_pdf_invalid_fitting: "imagesPdfInvalidFitting", images_pdf_already_running: "imagesPdfAlreadyRunning",
  pdf_images_password_invalid: "pdfImagesPasswordInvalid", pdf_images_security_error: "pdfImagesSecurityError",
  pdf_images_render_error: "pdfImagesRenderError", pdfium_unavailable: "pdfiumUnavailable",
  pdf_images_pages_required: "pdfImagesPagesRequired", pdf_images_invalid_pages: "pdfImagesInvalidPages",
  pdf_images_page_out_of_range: "pdfImagesPageOutOfRange", pdf_images_invalid_dpi: "pdfImagesInvalidDpi",
  pdf_images_invalid_quality: "pdfImagesInvalidQuality", pdf_images_already_running: "pdfImagesAlreadyRunning",
  pdf_images_invalid_range: "pdfImagesInvalidRange", pdf_images_insufficient_memory: "pdfImagesInsufficientMemory",
  pdf_images_destination_inaccessible: "pdfImagesDestinationInaccessible", pdf_images_insufficient_space: "pdfImagesInsufficientSpace",
  pdf_images_read_error: "pdfImagesReadError", pdf_images_write_error: "pdfImagesWriteError",
  pdf_images_encode_error: "pdfImagesEncodeError",
  open_image_invalid_path: "openImageInvalidPath", open_image_missing: "openImageMissing",
  open_image_empty: "openImageEmpty", open_image_unsupported: "openImageUnsupported",
  open_image_decode_failed: "openImageDecodeFailed",
};
function bytes(value: number) {
  if (!value) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const unit = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  const amount = value / 1024 ** unit;
  return `${amount >= 10 || unit === 0 ? amount.toFixed(0) : amount.toFixed(1)} ${units[unit]}`;
}
function nameFromPath(path: string) { return path.split(/[\\/]/).pop() || path; }
function joinSourceNames(names: string[]) { return names.join(" • "); }
function parseError(value: unknown) {
  if (typeof value === "object" && value) return value as { code?: string; message?: string };
  try { return JSON.parse(String(value)) as { code?: string; message?: string }; }
  catch { return { code: "unknown_error", message: String(value) }; }
}

type SplitMode = "extract" | "everyPage" | "ranges" | "everyN";
type SplitValidation = { errorKey?: TranslationKey; files: string[]; hiddenCount: number };

function splitStem(name: string) { return name.replace(/\.pdf$/i, "") || "document"; }
function pageRange(start: number, end: number) { return start === end ? `${start}` : `${start}-${end}`; }

function validateExtract(value: string, totalPages: number, stem: string): SplitValidation {
  if (!value.trim()) return { errorKey: "splitPagesRequired", files: [], hiddenCount: 0 };
  const pages = new Set<number>();
  for (const rawToken of value.split(",")) {
    const token = rawToken.trim();
    const match = token.match(/^(\d+)(?:\s*-\s*(\d+))?$/);
    if (!match) return { errorKey: "splitInvalidPages", files: [], hiddenCount: 0 };
    const start = Number(match[1]); const end = Number(match[2] ?? match[1]);
    if (start < 1 || end < start || end > totalPages) return { errorKey: "splitPageOutOfRange", files: [], hiddenCount: 0 };
    for (let page = start; page <= end; page += 1) pages.add(page);
  }
  return { files: [`${stem}-extracted.pdf`], hiddenCount: 0 };
}

function validateRanges(value: string, totalPages: number, stem: string): SplitValidation {
  const lines = value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  if (!lines.length) return { errorKey: "splitRangesRequired", files: [], hiddenCount: 0 };
  const ranges: string[] = [];
  for (const line of lines) {
    const match = line.match(/^(\d+)(?:\s*-\s*(\d+))?$/);
    if (!match) return { errorKey: "splitInvalidRanges", files: [], hiddenCount: 0 };
    const start = Number(match[1]); const end = Number(match[2] ?? match[1]);
    if (start < 1 || end < start || end > totalPages) return { errorKey: "splitPageOutOfRange", files: [], hiddenCount: 0 };
    ranges.push(`${stem}-pages-${pageRange(start, end)}.pdf`);
  }
  return { files: ranges, hiddenCount: 0 };
}

function previewEveryPage(totalPages: number, stem: string): SplitValidation {
  const visible = Math.min(totalPages, 12);
  return { files: Array.from({ length: visible }, (_, index) => `${stem}-page-${index + 1}.pdf`), hiddenCount: totalPages - visible };
}

function validateEveryN(value: string, totalPages: number, stem: string): SplitValidation {
  if (!/^\d+$/.test(value.trim()) || Number(value) < 1) return { errorKey: "splitInvalidN", files: [], hiddenCount: 0 };
  const count = Number(value);
  const files = [];
  for (let start = 1; start <= totalPages; start += count) files.push(`${stem}-pages-${pageRange(start, Math.min(start + count - 1, totalPages))}.pdf`);
  return { files, hiddenCount: 0 };
}

function SplitPdfPanel() {
  const { t } = useI18n();
  const { settings } = useSettings();
  const [file, setFile] = useState<PdfItem | null>(null);
  const [destination, setDestination] = useState<string | null>(null);
  const [mode, setMode] = useState<SplitMode>("extract");
  const [extractPages, setExtractPages] = useState("");
  const [ranges, setRanges] = useState("");
  const [everyN, setEveryN] = useState("2");
  const [notice, setNotice] = useState<string | null>(null);
  const [isSplitting, setIsSplitting] = useState(false);
  const [splitResult, setSplitResult] = useState<SplitResult | null>(null);
  const [splitProgress, setSplitProgress] = useState<SplitProgress>({ completed: 0, total: 0, percent: 0, currentFile: "" });

  const addPath = useCallback(async (path: string) => {
    if (isSplitting) return;
    setNotice(null); setSplitResult(null); setFile({ path, name: nameFromPath(path), size: 0, pages: 0, status: "reading" });
    try { setFile({ ...(await invoke<PdfInfo>("inspect_pdf", { path })), status: "ready" }); }
    catch (error) { const parsed = parseError(error); setFile((current) => current?.path === path ? { ...current, status: "failed", errorCode: parsed.code, errorMessage: parsed.message } : current); }
  }, [isSplitting]);

  useEffect(() => {
    let cleanup: (() => void) | undefined; let disposed = false;
    getCurrentWebview().onDragDropEvent((event) => { if (event.payload.type === "drop" && event.payload.paths[0]) void addPath(event.payload.paths[0]); })
      .then((unlisten) => { if (disposed) unlisten(); else cleanup = unlisten; });
    return () => { disposed = true; cleanup?.(); };
  }, [addPath]);

  useEffect(() => {
    let cleanup: (() => void) | undefined; let disposed = false;
    listen<SplitProgress>("pdf-split-progress", (event) => setSplitProgress(event.payload))
      .then((unlisten) => { if (disposed) unlisten(); else cleanup = unlisten; });
    return () => { disposed = true; cleanup?.(); };
  }, []);

  async function choosePdf() {
    const selected = await open({ multiple: false, directory: false, filters: [{ name: "PDF", extensions: ["pdf"] }] });
    if (typeof selected === "string") await addPath(selected);
  }
  async function chooseSplitDestination() {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === "string") setDestination(selected);
  }

  const validation = useMemo<SplitValidation>(() => {
    if (!file || file.status !== "ready") return { files: [], hiddenCount: 0 };
    const stem = splitStem(file.name);
    if (mode === "extract") return validateExtract(extractPages, file.pages, stem);
    if (mode === "everyPage") return previewEveryPage(file.pages, stem);
    if (mode === "ranges") return validateRanges(ranges, file.pages, stem);
    return validateEveryN(everyN, file.pages, stem);
  }, [everyN, extractPages, file, mode, ranges]);

  const errorMessage = file?.status === "failed"
    ? t(errorKeys[file.errorCode ?? "unknown_error"] ?? "unknownError")
    : validation.errorKey ? t(validation.errorKey, { pages: file?.pages ?? 0 }) : null;

  async function splitDocument() {
    if (!file || file.status !== "ready") { setNotice(t("splitAddValidPdf")); return; }
    if (validation.errorKey) { setNotice(t(validation.errorKey, { pages: file.pages })); return; }
    if (isSplitting) return;
    setIsSplitting(true); setNotice(null); setSplitResult(null);
    setSplitProgress({ completed: 0, total: validation.files.length, percent: 0, currentFile: validation.files[0] ?? "" });
    const startTime = Date.now();
    try {
      const response = await invoke<SplitResult>("split_pdf", { request: {
        path: file.path, destinationDir: destination, mode,
        pages: mode === "extract" ? extractPages : null,
        ranges: mode === "ranges" ? ranges : null,
        everyN: mode === "everyN" ? Number(everyN) : null,
      }});
      const durationMs = Date.now() - startTime;
      if (response.success) {
        if (response.files.length > 0) {
          const entries: ProcessingHistoryEntry[] = response.files.map((output) => createPdfProcessingHistoryEntry({
            tool: "Split PDF",
            sourceFileName: file.name,
            resultPath: output.path,
            success: true,
            originalSize: file.size,
            finalSize: output.size,
            durationMs,
          }));
          void appendProcessingHistoryEntries(entries, settings.enableProcessingHistory);
        } else {
          void appendProcessingHistoryEntry(createPdfProcessingHistoryEntry({
            tool: "Split PDF",
            sourceFileName: file.name,
            success: false,
            originalSize: file.size,
            durationMs,
            errorMessage: "Split PDF completed without creating an output file.",
          }), settings.enableProcessingHistory);
        }
        setSplitResult(response);
        setSplitProgress({ completed: response.files.length, total: response.files.length, percent: 100, currentFile: "" });
        setNotice(t("splitComplete", { count: response.files.length }));
      } else {
        void appendProcessingHistoryEntry(createPdfProcessingHistoryEntry({
          tool: "Split PDF",
          sourceFileName: file.name,
          success: false,
          originalSize: file.size,
          durationMs,
          errorMessage: response.errorMessage ?? (response.errorCode ? t(errorKeys[response.errorCode] ?? "unknownError", { pages: file.pages }) : t("unknownError")),
        }), settings.enableProcessingHistory);
        setNotice(response.errorCode ? t(errorKeys[response.errorCode] ?? "unknownError", { pages: file.pages }) : response.errorMessage ?? t("unknownError"));
      }
    } catch (error) {
      const parsed = parseError(error);
      void appendProcessingHistoryEntry(createPdfProcessingHistoryEntry({
        tool: "Split PDF",
        sourceFileName: file.name,
        success: false,
        originalSize: file.size,
        durationMs: Date.now() - startTime,
        errorMessage: parsed.message ?? (parsed.code ? t(errorKeys[parsed.code] ?? "unknownError", { pages: file.pages }) : t("unknownError")),
      }), settings.enableProcessingHistory);
      setNotice(parsed.code ? t(errorKeys[parsed.code] ?? "unknownError", { pages: file.pages }) : parsed.message ?? t("unknownError"));
    } finally { setIsSplitting(false); }
  }

  const modes: { id: SplitMode; key: TranslationKey; hint: TranslationKey; icon: typeof Scissors }[] = [
    { id: "extract", key: "extractPages", hint: "extractPagesHint", icon: Scissors },
    { id: "everyPage", key: "splitEveryPage", hint: "splitEveryPageHint", icon: Files },
    { id: "ranges", key: "splitByRange", hint: "splitByRangeHint", icon: ListOrdered },
    { id: "everyN", key: "splitEveryNPages", hint: "splitEveryNPagesHint", icon: Layers3 },
  ];

  return <>
    <section className="pdf-drop-zone split-drop-zone" onClick={() => !isSplitting && void choosePdf()}><div className="compress-drop-icon"><UploadCloud size={28}/></div><div><h2>{t("splitDropTitle")}</h2><p>{t("splitDropSubtitle")}</p></div><button className="primary-button" disabled={isSplitting} onClick={(event) => { event.stopPropagation(); void choosePdf(); }}><FilePlus2 size={17}/>{file ? t("replacePdf") : t("addPdf")}</button></section>
    <section className="compress-panel split-file-panel">
      <div className="compress-panel-header"><div><h2>{t("selectedPdf")}</h2><span>{t("singlePdfOnly")}</span></div></div>
      {!file ? <div className="empty-image-list"><Files size={25}/><strong>{t("noPdfSelected")}</strong><p>{t("splitAddValidPdf")}</p></div> : <div className={`split-file-row ${file.status}`}><Files size={24}/><span className="pdf-name" title={file.path}>{file.name}{file.status === "failed" && <small>{errorMessage}</small>}</span><span><small>{t("fileSize")}</small><strong>{bytes(file.size)}</strong></span><span><small>{t("totalPages")}</small><strong>{file.pages || "—"}</strong></span><button className="secondary-button" disabled={isSplitting} onClick={() => void choosePdf()}>{t("replacePdf")}</button><button className="icon-button remove-button" aria-label={t("removeFile")} disabled={isSplitting} onClick={() => { setFile(null); setNotice(null); setSplitResult(null); }}><Trash2 size={14}/></button></div>}
    </section>
    <section className="compress-panel split-modes-panel"><div><h2>{t("splitMode")}</h2><p>{t("splitModeHint")}</p></div><div className="split-mode-tabs">{modes.map((entry) => { const Icon = entry.icon; return <button key={entry.id} className={mode === entry.id ? "active" : ""} disabled={isSplitting} onClick={() => { setMode(entry.id); setNotice(null); setSplitResult(null); }}><Icon size={16}/><span><strong>{t(entry.key)}</strong><small>{t(entry.hint)}</small></span></button>; })}</div>
      <div className="split-mode-fields">
        {mode === "extract" && <label><span>{t("pagesToExtract")}</span><input value={extractPages} disabled={isSplitting} onChange={(event) => { setExtractPages(event.target.value); setNotice(null); setSplitResult(null); }} placeholder="1, 3, 5-8, 12"/><small>{t("extractInputHint")}</small></label>}
        {mode === "everyPage" && <div className="split-mode-note"><Files size={18}/><span><strong>{t("splitEveryPage")}</strong><small>{t("splitEveryPageDescription", { pages: file?.pages ?? 0 })}</small></span></div>}
        {mode === "ranges" && <label><span>{t("pageRanges")}</span><textarea value={ranges} disabled={isSplitting} onChange={(event) => { setRanges(event.target.value); setNotice(null); setSplitResult(null); }} placeholder={"1-3\n4-7\n8-10"}/><small>{t("rangeInputHint")}</small></label>}
        {mode === "everyN" && <label><span>{t("pagesPerFile")}</span><input type="number" min="1" step="1" value={everyN} disabled={isSplitting} onChange={(event) => { setEveryN(event.target.value); setNotice(null); setSplitResult(null); }} placeholder="2"/><small>{t("everyNInputHint")}</small></label>}
        {errorMessage && file?.status !== "failed" && <p className="split-validation-error">{errorMessage}</p>}
      </div>
    </section>
    <section className="compress-panel split-bottom-grid"><div className="split-preview"><div><h2>{t("filesPreview")}</h2><p>{t("filesPreviewHint")}</p></div>{validation.files.length ? <ol>{validation.files.map((name) => <li key={name}>{name}</li>)}{validation.hiddenCount > 0 && <li className="more-files">{t("moreFiles", { count: validation.hiddenCount })}</li>}</ol> : <div className="split-preview-empty">{t("previewUnavailable")}</div>}</div><div className="pdf-destination split-destination"><MapPin size={17}/><span><small>{t("destinationFolder")}</small><strong title={destination ?? ""}>{destination ?? t("sameFolder")}</strong></span><button className="secondary-button" disabled={isSplitting} onClick={() => void chooseSplitDestination()}><FolderOpen size={15}/>{t("chooseFolder")}</button></div></section>
    {isSplitting && <section className="compress-panel split-progress"><div><span><strong>{t("splittingPdf")}</strong><small>{t("creatingFile")}: <b dir="ltr">{splitProgress.currentFile || "—"}</b></small></span><span>{splitProgress.completed}/{splitProgress.total} · {splitProgress.percent}%</span></div><progress max="100" value={splitProgress.percent}/></section>}
    {splitResult?.success && splitResult.files.length > 0 && <section className="compress-panel split-results"><div className="split-results-header"><div><CheckCircle2 size={22}/><span><strong>{t("splitResults")}</strong><small>{t("splitComplete", { count: splitResult.files.length })}</small></span></div><div className="split-results-summary"><span><small>{t("filesCreated")}</small><strong>{splitResult.files.length}</strong></span><span><small>{t("pagesExtracted")}</small><strong>{splitResult.totalPages}</strong></span><span><small>{t("destinationFolder")}</small><strong dir="ltr" title={splitResult.destinationDir}>{splitResult.destinationDir}</strong></span></div></div><div className="split-result-list"><div className="split-result-head"><span>{t("fileName")}</span><span>{t("pages")}</span><span>{t("fileSize")}</span><span></span></div>{splitResult.files.map((output) => <div className="split-result-row" key={output.path}><span title={output.path}>{output.name}</span><span>{output.pages}</span><span>{bytes(output.size)}</span><span><button className="text-button" onClick={() => void openPath(output.path)}>{t("openFile")}</button><button className="text-button" onClick={() => void revealItemInDir(output.path)}>{t("openFolder")}</button></span></div>)}</div></section>}
    {notice && <div className={splitResult?.success ? "split-info" : "page-error"}>{notice}</div>}
    <div className="pdf-main-action"><button className="primary-button" onClick={() => void splitDocument()} disabled={file?.status === "reading" || isSplitting}>{isSplitting ? <LoaderCircle className="spin" size={17}/> : <Scissors size={17}/>} {isSplitting ? t("splittingPdf") : t("splitPdf")}</button></div>
  </>;
}

interface OrganizePageItem { id: number; originalPage: number; rotation: number; }

function moveSelectedPages(pages: OrganizePageItem[], selected: Set<number>, direction: -1 | 1) {
  const next = [...pages];
  if (direction < 0) {
    for (let index = 1; index < next.length; index += 1) {
      if (selected.has(next[index].id) && !selected.has(next[index - 1].id)) [next[index - 1], next[index]] = [next[index], next[index - 1]];
    }
  } else {
    for (let index = next.length - 2; index >= 0; index -= 1) {
      if (selected.has(next[index].id) && !selected.has(next[index + 1].id)) [next[index], next[index + 1]] = [next[index + 1], next[index]];
    }
  }
  return next;
}

function OrganizePagesPanel() {
  const { t } = useI18n();
  const { settings } = useSettings();
  const [file, setFile] = useState<PdfItem | null>(null);
  const [pages, setPages] = useState<OrganizePageItem[]>([]);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [destination, setDestination] = useState<string | null>(null);
  const [outputName, setOutputName] = useState("organized-document");
  const [notice, setNotice] = useState<string | null>(null);
  const draggingPage = useRef<number | null>(null);
  const nextPageId = useRef(1);
  const [isOrganizing, setIsOrganizing] = useState(false);
  const [result, setResult] = useState<OrganizeResult | null>(null);

  const addPath = useCallback(async (path: string) => {
    if (isOrganizing) return;
    setNotice(null); setResult(null); setSelected(new Set()); setPages([]);
    setFile({ path, name: nameFromPath(path), size: 0, pages: 0, status: "reading" });
    try {
      const info = await invoke<PdfInfo>("inspect_pdf", { path });
      setFile({ ...info, status: "ready" });
      setPages(Array.from({ length: info.pages }, (_, index) => ({ id: index + 1, originalPage: index + 1, rotation: 0 })));
      nextPageId.current = info.pages + 1;
      setOutputName(`${splitStem(info.name)}-organized`);
    } catch (error) {
      const parsed = parseError(error);
      setFile((current) => current?.path === path ? { ...current, status: "failed", errorCode: parsed.code, errorMessage: parsed.message } : current);
    }
  }, [isOrganizing]);

  useEffect(() => {
    let cleanup: (() => void) | undefined; let disposed = false;
    getCurrentWebview().onDragDropEvent((event) => { if (event.payload.type === "drop" && event.payload.paths[0]) void addPath(event.payload.paths[0]); })
      .then((unlisten) => { if (disposed) unlisten(); else cleanup = unlisten; });
    return () => { disposed = true; cleanup?.(); };
  }, [addPath]);

  async function choosePdf() {
    const picked = await open({ multiple: false, directory: false, filters: [{ name: "PDF", extensions: ["pdf"] }] });
    if (typeof picked === "string") await addPath(picked);
  }
  async function chooseDestination() {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") setDestination(picked);
  }
  function togglePage(id: number) {
    setSelected((current) => { const next = new Set(current); if (next.has(id)) next.delete(id); else next.add(id); return next; });
  }
  function dropPage(targetId: number) {
    const sourceId = draggingPage.current; draggingPage.current = null;
    if (sourceId === null || sourceId === targetId) return;
    setPages((current) => {
      const next = [...current]; const from = next.findIndex((page) => page.id === sourceId);
      const [entry] = next.splice(from, 1); const to = next.findIndex((page) => page.id === targetId);
      next.splice(to, 0, entry); return next;
    });
    setNotice(null); setResult(null);
  }
  function moveSelection(direction: -1 | 1) {
    setPages((current) => moveSelectedPages(current, selected, direction));
    setNotice(null); setResult(null);
  }
  function rotate(delta: number) {
    if (!selected.size) { setNotice(t("organizeSelectPages")); return; }
    setPages((current) => current.map((page) => selected.has(page.id) ? { ...page, rotation: (page.rotation + delta + 360) % 360 } : page));
    setNotice(null); setResult(null);
  }
  function removeSelected() {
    if (!selected.size) { setNotice(t("organizeSelectPages")); return; }
    setPages((current) => current.filter((page) => !selected.has(page.id))); setSelected(new Set()); setNotice(null); setResult(null);
  }
  function duplicateSelected() {
    if (!selected.size) { setNotice(t("organizeSelectPages")); return; }
    const duplicated = new Set<number>();
    setPages((current) => current.flatMap((page) => {
      if (!selected.has(page.id)) return [page];
      const copy = { ...page, id: nextPageId.current++ };
      duplicated.add(copy.id);
      return [page, copy];
    }));
    setSelected(duplicated); setNotice(null); setResult(null);
  }
  function resetPages() {
    if (!file || file.status !== "ready") return;
    setPages(Array.from({ length: file.pages }, (_, index) => ({ id: index + 1, originalPage: index + 1, rotation: 0 })));
    nextPageId.current = file.pages + 1;
    setSelected(new Set()); setNotice(null); setResult(null);
  }
  async function saveOrganizedPdf() {
    if (!file || file.status !== "ready") { setNotice(t("organizeAddValidPdf")); return; }
    if (!pages.length) { setNotice(t("organizeNeedsPage")); return; }
    if (!outputName.trim()) { setNotice(t("organizeNameRequired")); return; }
    if (isOrganizing) return;
    setIsOrganizing(true); setNotice(null); setResult(null);
    const startTime = Date.now();
    try {
      const response = await invoke<OrganizeResult>("organize_pdf", { request: {
        path: file.path, destinationDir: destination, outputName,
        pages: pages.map((page) => ({ sourcePage: page.originalPage, rotation: page.rotation })),
      }});
      const durationMs = Date.now() - startTime;
      void appendProcessingHistoryEntry(createPdfProcessingHistoryEntry({
        tool: "Organize Pages",
        sourceFileName: file.name,
        resultPath: response.outputPath,
        success: response.success,
        originalSize: file.size,
        finalSize: response.newSize,
        durationMs,
        errorMessage: response.success ? undefined : response.errorMessage ?? (response.errorCode ? t(errorKeys[response.errorCode] ?? "unknownError") : t("unknownError")),
      }), settings.enableProcessingHistory);
      if (response.success) { setResult(response); setNotice(t("organizeComplete")); }
      else setNotice(response.errorCode ? t(errorKeys[response.errorCode] ?? "unknownError") : response.errorMessage ?? t("unknownError"));
    } catch (error) {
      const parsed = parseError(error);
      void appendProcessingHistoryEntry(createPdfProcessingHistoryEntry({
        tool: "Organize Pages",
        sourceFileName: file.name,
        success: false,
        originalSize: file.size,
        durationMs: Date.now() - startTime,
        errorMessage: parsed.message ?? (parsed.code ? t(errorKeys[parsed.code] ?? "unknownError") : t("unknownError")),
      }), settings.enableProcessingHistory);
      setNotice(parsed.code ? t(errorKeys[parsed.code] ?? "unknownError") : parsed.message ?? t("unknownError"));
    } finally { setIsOrganizing(false); }
  }

  const fileError = file?.status === "failed" ? t(errorKeys[file.errorCode ?? "unknown_error"] ?? "unknownError") : null;
  return <>
    <section className="pdf-drop-zone organize-drop-zone" onClick={() => !isOrganizing && void choosePdf()}><div className="compress-drop-icon"><UploadCloud size={28}/></div><div><h2>{t("organizeDropTitle")}</h2><p>{t("organizeDropSubtitle")}</p></div><button className="primary-button" disabled={isOrganizing} onClick={(event) => { event.stopPropagation(); void choosePdf(); }}><FilePlus2 size={17}/>{file ? t("replacePdf") : t("addPdf")}</button></section>
    <section className="compress-panel organize-file-panel"><div className="compress-panel-header"><div><h2>{t("selectedPdf")}</h2><span>{t("singlePdfOnly")}</span></div></div>{!file ? <div className="empty-image-list"><Files size={25}/><strong>{t("noPdfSelected")}</strong><p>{t("organizeAddValidPdf")}</p></div> : <div className={`split-file-row ${file.status}`}><Files size={24}/><span className="pdf-name" title={file.path}>{file.name}{fileError && <small>{fileError}</small>}</span><span><small>{t("fileSize")}</small><strong>{bytes(file.size)}</strong></span><span><small>{t("totalPages")}</small><strong>{file.pages || "—"}</strong></span><button className="secondary-button" disabled={isOrganizing} onClick={() => void choosePdf()}>{t("replacePdf")}</button><button className="icon-button remove-button" aria-label={t("removeFile")} disabled={isOrganizing} onClick={() => { setFile(null); setPages([]); setSelected(new Set()); setNotice(null); setResult(null); }}><Trash2 size={14}/></button></div>}</section>
    {file?.status === "ready" && <section className="compress-panel organize-workspace"><div className="organize-toolbar"><div><strong>{t("organizePagesSelected", { selected: selected.size, total: pages.length })}</strong><small>{t("organizeDragHint")}</small></div><div className="organize-actions"><button disabled={!selected.size || isOrganizing} onClick={() => moveSelection(-1)}><ArrowLeft size={14}/>{t("moveLeft")}</button><button disabled={!selected.size || isOrganizing} onClick={() => moveSelection(1)}><ArrowRight size={14}/>{t("moveRight")}</button><i></i><button disabled={!selected.size || isOrganizing} onClick={() => rotate(-90)}><RotateCcw size={14}/>{t("rotateLeft")}</button><button disabled={!selected.size || isOrganizing} onClick={() => rotate(90)}><RotateCw size={14}/>{t("rotateRight")}</button><button disabled={!selected.size || isOrganizing} onClick={() => rotate(180)}><RotateCw size={14}/>{t("rotate180")}</button><button disabled={!selected.size || isOrganizing} onClick={duplicateSelected}><FilePlus2 size={14}/>{t("duplicatePage")}</button><button className="danger" disabled={!selected.size || isOrganizing} onClick={removeSelected}><Trash2 size={14}/>{t("deletePages")}</button><button disabled={isOrganizing} onClick={resetPages}><Undo2 size={14}/>{t("reset")}</button></div></div>
      {!pages.length ? <div className="organize-empty"><Trash2 size={24}/><strong>{t("allPagesDeleted")}</strong><button className="secondary-button" disabled={isOrganizing} onClick={resetPages}>{t("reset")}</button></div> : <div className="organize-page-grid">{pages.map((page, index) => <article key={page.id} className={`organize-page-card ${selected.has(page.id) ? "selected" : ""}`} draggable={!isOrganizing} onDragStart={() => { draggingPage.current = page.id; }} onDragOver={(event) => event.preventDefault()} onDrop={() => dropPage(page.id)} onClick={() => !isOrganizing && togglePage(page.id)}><div className="organize-page-position">{index + 1}</div><div className="organize-page-paper"><div style={{ transform: `rotate(${page.rotation}deg)` }}><span>{page.originalPage}</span><b></b><b></b><b></b></div></div><label onClick={(event) => event.stopPropagation()}><input type="checkbox" disabled={isOrganizing} checked={selected.has(page.id)} onChange={() => togglePage(page.id)}/><span>{t("pageNumber", { page: page.originalPage })}</span></label>{page.rotation !== 0 && <small>{page.rotation}°</small>}<GripVertical size={14}/></article>)}</div>}
    </section>}
    <section className="compress-panel organize-output"><div><h2>{t("organizeOutput")}</h2><p>{t("organizeOutputHint")}</p></div><label><span>{t("finalFileName")}</span><div><input value={outputName} disabled={isOrganizing} onChange={(event) => { setOutputName(event.target.value); setNotice(null); setResult(null); }}/><b>.pdf</b></div></label><div className="pdf-destination"><MapPin size={17}/><span><small>{t("destinationFolder")}</small><strong title={destination ?? ""}>{destination ?? t("sameFolder")}</strong></span><button className="secondary-button" disabled={isOrganizing} onClick={() => void chooseDestination()}><FolderOpen size={15}/>{t("chooseFolder")}</button></div></section>
    {result?.success && result.outputPath && <section className="compress-panel organize-result"><CheckCircle2 size={26}/><div><h2>{t("organizeComplete")}</h2><div><span><small>{t("initialPageCount")}</small><strong>{result.initialPages}</strong></span><span><small>{t("finalPageCount")}</small><strong>{result.finalPages}</strong></span><span><small>{t("createdFileSize")}</small><strong>{bytes(result.newSize ?? 0)}</strong></span><span className="organize-result-path"><small>{t("createdFile")}</small><strong title={result.outputPath}>{result.outputPath}</strong></span></div></div><aside><button className="secondary-button" onClick={() => void openPath(result.outputPath!)}>{t("openFile")}</button><button className="primary-button" onClick={() => void revealItemInDir(result.outputPath!)}><FolderOpen size={15}/>{t("openFolder")}</button></aside></section>}
    {notice && <div className={result?.success ? "split-info" : "page-error"}>{notice}</div>}
    <div className="pdf-main-action"><button className="primary-button" disabled={file?.status === "reading" || isOrganizing} onClick={() => void saveOrganizedPdf()}>{isOrganizing ? <LoaderCircle className="spin" size={17}/> : <Save size={17}/>} {isOrganizing ? t("organizingPdf") : t("saveOrganizedPdf")}</button></div>
  </>;
}

type PdfPageSize = "a4" | "a3" | "letter" | "legal" | "fitImage" | "custom";
type PdfOrientation = "auto" | "portrait" | "landscape";
type PdfMargin = "none" | "small" | "medium" | "large" | "custom";
type PdfImageFitting = "fit" | "fill" | "stretch";

function ImagesToPdfPanel() {
  const { t } = useI18n();
  const { settings } = useSettings();
  const [items, setItems] = useState<PdfImageItem[]>([]);
  const [destination, setDestination] = useState<string | null>(null);
  const [outputName, setOutputName] = useState("images-document");
  const [pageSize, setPageSize] = useState<PdfPageSize>("a4");
  const [orientation, setOrientation] = useState<PdfOrientation>("auto");
  const [margin, setMargin] = useState<PdfMargin>("small");
  const [fitting, setFitting] = useState<PdfImageFitting>("fit");
  const [customWidth, setCustomWidth] = useState("210");
  const [customHeight, setCustomHeight] = useState("297");
  const [customMargin, setCustomMargin] = useState("10");
  const [notice, setNotice] = useState<string | null>(null);
  const [background, setBackground] = useState<"white" | "black" | "custom">("white");
  const [customBackground, setCustomBackground] = useState("#ffffff");
  const [quality, setQuality] = useState(90);
  const [isCreating, setIsCreating] = useState(false);
  const [progress, setProgress] = useState<ImagesPdfProgress>({ completed: 0, total: 0, percent: 0, currentFile: "" });
  const [result, setResult] = useState<ImagesPdfResult | null>(null);
  const draggingImage = useRef<string | null>(null);

  const addPaths = useCallback(async (paths: string[]) => {
    const unique = paths.filter((path, index) => paths.indexOf(path) === index && !items.some((item) => item.path === path));
    if (!unique.length) return;
    if (isCreating) return;
    setNotice(null); setResult(null);
    setItems((current) => [...current, ...unique.map((path) => ({ path, name: nameFromPath(path), format: "—", width: 0, height: 0, size: 0, status: "reading" as const }))]);
    await Promise.all(unique.map(async (path) => {
      try {
        const info = await invoke<PdfImageInfo>("inspect_convertible_image", { path });
        setItems((current) => current.map((item) => item.path === path ? { ...info, status: "ready" } : item));
      } catch (error) {
        const parsed = parseError(error);
        setItems((current) => current.map((item) => item.path === path ? { ...item, status: "failed", errorCode: parsed.code, errorMessage: parsed.message } : item));
      }
    }));
  }, [isCreating, items]);

  useEffect(() => {
    let cleanup: (() => void) | undefined; let disposed = false;
    getCurrentWebview().onDragDropEvent((event) => { if (event.payload.type === "drop") void addPaths(event.payload.paths); })
      .then((unlisten) => { if (disposed) unlisten(); else cleanup = unlisten; });
    return () => { disposed = true; cleanup?.(); };
  }, [addPaths]);

  useEffect(() => {
    let cleanup: (() => void) | undefined; let disposed = false;
    listen<ImagesPdfProgress>("images-pdf-progress", (event) => setProgress(event.payload))
      .then((unlisten) => { if (disposed) unlisten(); else cleanup = unlisten; });
    return () => { disposed = true; cleanup?.(); };
  }, []);

  async function chooseImages() {
    const picked = await open({ multiple: true, directory: false, filters: [{ name: "Images", extensions: ["jpg", "jpeg", "png", "webp", "bmp", "tif", "tiff"] }] });
    if (picked) await addPaths(Array.isArray(picked) ? picked : [picked]);
  }
  async function chooseDestination() {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") setDestination(picked);
  }
  function move(index: number, direction: -1 | 1) {
    const target = index + direction; if (target < 0 || target >= items.length) return;
    setItems((current) => { const next = [...current]; [next[index], next[target]] = [next[target], next[index]]; return next; });
    setNotice(null);
  }
  function dropOn(targetPath: string) {
    const sourcePath = draggingImage.current; draggingImage.current = null;
    if (!sourcePath || sourcePath === targetPath) return;
    setItems((current) => {
      const next = [...current]; const from = next.findIndex((item) => item.path === sourcePath);
      const [entry] = next.splice(from, 1); const to = next.findIndex((item) => item.path === targetPath);
      next.splice(to, 0, entry); return next;
    });
    setNotice(null);
  }
  async function createPdf() {
    if (items.some((item) => item.status === "reading")) { setNotice(t("imagesPdfWaitForImages")); return; }
    if (items.some((item) => item.status === "failed")) { setNotice(t("imagesPdfRemoveInvalid")); return; }
    const valid = items.filter((item) => item.status === "ready");
    if (!valid.length) { setNotice(t("imagesPdfNeedImages")); return; }
    if (!outputName.trim()) { setNotice(t("imagesPdfNameRequired")); return; }
    if (pageSize === "custom" && (!(Number(customWidth) > 0) || !(Number(customHeight) > 0))) { setNotice(t("imagesPdfInvalidPageSize")); return; }
    if (margin === "custom" && (!(Number(customMargin) >= 0) || customMargin.trim() === "")) { setNotice(t("imagesPdfInvalidMargin")); return; }
    if (quality < 10 || quality > 100) { setNotice(t("imagesPdfInvalidQuality")); return; }
    if (isCreating) return;
    setIsCreating(true); setNotice(null); setResult(null);
    setProgress({ completed: 0, total: valid.length, percent: 0, currentFile: valid[0].name });
    const sourceFileName = joinSourceNames(valid.map((item) => item.name));
    const originalSize = valid.reduce((total, item) => total + item.size, 0);
    const startTime = Date.now();
    try {
      const response = await invoke<ImagesPdfResult>("images_to_pdf", { request: {
        paths: valid.map((item) => item.path), destinationDir: destination, outputName, pageSize,
        customWidthMm: Number(customWidth), customHeightMm: Number(customHeight), orientation,
        margin, customMarginMm: Number(customMargin), fitting,
        backgroundColor: background === "white" ? "#ffffff" : background === "black" ? "#000000" : customBackground,
        quality,
      }});
      const durationMs = Date.now() - startTime;
      void appendProcessingHistoryEntry(createPdfProcessingHistoryEntry({
        tool: "Images to PDF",
        sourceFileName,
        resultPath: response.outputPath,
        success: response.success,
        originalSize,
        finalSize: response.newSize,
        durationMs,
        errorMessage: response.success ? undefined : response.errorMessage ?? (response.errorCode ? t(errorKeys[response.errorCode] ?? "unknownError") : t("unknownError")),
      }), settings.enableProcessingHistory);
      if (response.success) { setResult(response); setProgress((current) => ({ ...current, completed: response.pageCount, total: response.pageCount, percent: 100 })); setNotice(t("imagesPdfComplete")); }
      else setNotice(response.errorCode ? t(errorKeys[response.errorCode] ?? "unknownError") : response.errorMessage ?? t("unknownError"));
    } catch (error) {
      const parsed = parseError(error);
      void appendProcessingHistoryEntry(createPdfProcessingHistoryEntry({
        tool: "Images to PDF",
        sourceFileName,
        success: false,
        originalSize,
        durationMs: Date.now() - startTime,
        errorMessage: parsed.message ?? (parsed.code ? t(errorKeys[parsed.code] ?? "unknownError") : t("unknownError")),
      }), settings.enableProcessingHistory);
      setNotice(parsed.code ? t(errorKeys[parsed.code] ?? "unknownError") : parsed.message ?? t("unknownError"));
    } finally { setIsCreating(false); }
  }

  const pageSizes: { id: PdfPageSize; key: TranslationKey; detail: string }[] = [
    { id: "a4", key: "pageSizeA4", detail: "210 × 297 mm" }, { id: "a3", key: "pageSizeA3", detail: "297 × 420 mm" },
    { id: "letter", key: "pageSizeLetter", detail: "8.5 × 11 in" }, { id: "legal", key: "pageSizeLegal", detail: "8.5 × 14 in" },
    { id: "fitImage", key: "fitToImage", detail: t("onePagePerImage") }, { id: "custom", key: "custom", detail: t("customDimensions") },
  ];
  const orientations: { id: PdfOrientation; key: TranslationKey }[] = [{ id: "auto", key: "orientationAuto" }, { id: "portrait", key: "orientationPortrait" }, { id: "landscape", key: "orientationLandscape" }];
  const margins: { id: PdfMargin; key: TranslationKey }[] = [{ id: "none", key: "marginNone" }, { id: "small", key: "marginSmall" }, { id: "medium", key: "marginMedium" }, { id: "large", key: "marginLarge" }, { id: "custom", key: "custom" }];
  const fittings: { id: PdfImageFitting; key: TranslationKey; hint: TranslationKey }[] = [{ id: "fit", key: "fitMethod", hint: "imagesPdfFitHint" }, { id: "fill", key: "fillMethod", hint: "imagesPdfFillHint" }, { id: "stretch", key: "stretchMethod", hint: "imagesPdfStretchHint" }];

  return <>
    <section className="pdf-drop-zone images-pdf-drop" onClick={() => void chooseImages()}><div className="compress-drop-icon"><UploadCloud size={28}/></div><div><h2>{t("imagesPdfDropTitle")}</h2><p>{t("imagesPdfDropSubtitle")}</p></div><button className="primary-button" onClick={(event) => { event.stopPropagation(); void chooseImages(); }}><ImagePlus size={17}/>{t("addImages")}</button></section>
    <section className="compress-panel images-pdf-list-panel"><div className="compress-panel-header"><div><h2>{t("selectedImages")}</h2><span>{t("imageCount", { count: items.length })}</span></div>{items.length > 0 && <button className="text-button danger" onClick={() => { setItems([]); setNotice(null); }}><Trash2 size={14}/>{t("clearAll")}</button>}</div>
      {!items.length ? <div className="empty-image-list"><Images size={25}/><strong>{t("noImages")}</strong><p>{t("imagesPdfNoImagesHint")}</p></div> : <div className="images-pdf-list"><div className="images-pdf-list-head"><span></span><span>{t("fileName")}</span><span>{t("format")}</span><span>{t("dimensions")}</span><span>{t("fileSize")}</span><span>{t("order")}</span><span></span></div>{items.map((item, index) => <div key={item.path} className={`images-pdf-row ${item.status}`} draggable onDragStart={() => { draggingImage.current = item.path; }} onDragOver={(event) => event.preventDefault()} onDrop={() => dropOn(item.path)}><div className="images-pdf-thumb"><FileImage size={18}/>{item.status === "ready" && <img src={convertFileSrc(item.path)} alt="" onError={(event) => { event.currentTarget.style.display = "none"; }}/>}<span>{index + 1}</span></div><span className="pdf-name" title={item.path}>{item.name}{item.status === "failed" && <small>{t(errorKeys[item.errorCode ?? "unknown_error"] ?? "unknownError")}</small>}</span><span>{item.format}</span><span>{item.width > 0 ? `${item.width} × ${item.height}` : "—"}</span><span>{bytes(item.size)}</span><span className="order-buttons"><button aria-label={t("moveUp")} disabled={index === 0} onClick={() => move(index, -1)}><ArrowUp size={13}/></button><button aria-label={t("moveDown")} disabled={index === items.length - 1} onClick={() => move(index, 1)}><ArrowDown size={13}/></button></span><button className="icon-button remove-button" aria-label={t("removeFile")} onClick={() => setItems((current) => current.filter((entry) => entry.path !== item.path))}><Trash2 size={14}/></button></div>)}</div>}
    </section>
    <section className="images-pdf-settings"><div className="compress-panel images-pdf-page-settings"><div><h2>{t("pageSize")}</h2><p>{t("pageSizeHint")}</p></div><div className="images-pdf-page-sizes">{pageSizes.map((option) => <button key={option.id} className={pageSize === option.id ? "active" : ""} onClick={() => { setPageSize(option.id); setNotice(null); }}><strong>{t(option.key)}</strong><small>{option.detail}</small></button>)}</div>{pageSize === "custom" && <div className="images-pdf-custom-fields"><label><span>{t("width")}</span><div><input type="number" min="1" value={customWidth} onChange={(event) => setCustomWidth(event.target.value)}/><b>mm</b></div></label><span>×</span><label><span>{t("height")}</span><div><input type="number" min="1" value={customHeight} onChange={(event) => setCustomHeight(event.target.value)}/><b>mm</b></div></label></div>}</div>
      <div className="compress-panel images-pdf-options"><div><h2>{t("pageOptions")}</h2><p>{t("pageOptionsHint")}</p></div><div className="images-pdf-option-group"><strong>{t("orientation")}</strong><div>{orientations.map((option) => <button key={option.id} className={orientation === option.id ? "active" : ""} onClick={() => setOrientation(option.id)}>{t(option.key)}</button>)}</div></div><div className="images-pdf-option-group"><strong>{t("margins")}</strong><div className="margin-options">{margins.map((option) => <button key={option.id} className={margin === option.id ? "active" : ""} onClick={() => setMargin(option.id)}>{t(option.key)}</button>)}</div>{margin === "custom" && <label className="custom-margin"><input type="number" min="0" value={customMargin} onChange={(event) => setCustomMargin(event.target.value)}/><b>mm</b></label>}</div></div>
    </section>
    <section className="compress-panel images-pdf-fitting"><div><h2>{t("imageFitting")}</h2><p>{t("imageFittingHint")}</p></div><div>{fittings.map((option) => <button key={option.id} disabled={isCreating} className={fitting === option.id ? "active" : ""} onClick={() => setFitting(option.id)}><strong>{t(option.key)}</strong><small>{t(option.hint)}</small></button>)}</div>{fitting === "stretch" && <div className="images-pdf-stretch-warning">{t("imagesPdfStretchWarning")}</div>}</section>
    <section className="compress-panel images-pdf-render-options"><div><h2>{t("imageRendering")}</h2><p>{t("imageRenderingHint")}</p></div><div className="images-pdf-background"><strong>{t("pageBackground")}</strong><div><button disabled={isCreating} className={background === "white" ? "active" : ""} onClick={() => setBackground("white")}><i style={{ background: "#fff" }}></i>{t("backgroundWhite")}</button><button disabled={isCreating} className={background === "black" ? "active" : ""} onClick={() => setBackground("black")}><i style={{ background: "#000" }}></i>{t("backgroundBlack")}</button><button disabled={isCreating} className={background === "custom" ? "active" : ""} onClick={() => setBackground("custom")}><i style={{ background: customBackground }}></i>{t("backgroundCustom")}</button></div>{background === "custom" && <label><input type="color" value={customBackground} disabled={isCreating} onChange={(event) => setCustomBackground(event.target.value)}/><code>{customBackground}</code></label>}</div><div className="images-pdf-quality"><div><strong>{t("imageQuality")}</strong><b>{quality}</b></div><input type="range" min="10" max="100" value={quality} disabled={isCreating} onChange={(event) => setQuality(Number(event.target.value))}/><small>{t("imagesPdfQualityHint")}</small></div></section>
    <section className="compress-panel organize-output images-pdf-output"><div><h2>{t("pdfOutputSettings")}</h2><p>{t("imagesPdfOutputHint")}</p></div><label><span>{t("pdfFileName")}</span><div><input value={outputName} onChange={(event) => { setOutputName(event.target.value); setNotice(null); }}/><b>.pdf</b></div></label><div className="pdf-destination"><MapPin size={17}/><span><small>{t("destinationFolder")}</small><strong title={destination ?? ""}>{destination ?? t("sameFolder")}</strong></span><button className="secondary-button" onClick={() => void chooseDestination()}><FolderOpen size={15}/>{t("chooseFolder")}</button></div></section>
    {isCreating && <section className="compress-panel split-progress"><div><span><strong>{t("creatingPdf")}</strong><small>{t("processingImage")}: <b dir="ltr">{progress.currentFile}</b></small></span><span>{progress.completed}/{progress.total} · {progress.percent}%</span></div><progress max="100" value={progress.percent}/></section>}
    {result?.success && result.outputPath && <section className="compress-panel organize-result images-pdf-result"><CheckCircle2 size={26}/><div><h2>{t("imagesPdfComplete")}</h2><div><span><small>{t("imagesAdded")}</small><strong>{result.imageCount}</strong></span><span><small>{t("pagesCreated")}</small><strong>{result.pageCount}</strong></span><span><small>{t("pdfSize")}</small><strong>{bytes(result.newSize ?? 0)}</strong></span><span><small>{t("pageFormatUsed")}</small><strong>{result.pageSizeUsed}</strong></span><span className="organize-result-path"><small>{t("filePath")}</small><strong title={result.outputPath}>{result.outputPath}</strong></span></div></div><aside><button className="secondary-button" onClick={() => void openPath(result.outputPath!)}>{t("openFile")}</button><button className="primary-button" onClick={() => void revealItemInDir(result.outputPath!)}><FolderOpen size={15}/>{t("openFolder")}</button></aside></section>}
    {notice && <div className={result?.success ? "split-info" : "page-error"}>{notice}</div>}
    <div className="pdf-main-action"><button className="primary-button" disabled={isCreating || items.some((item) => item.status === "reading")} onClick={() => void createPdf()}>{isCreating ? <LoaderCircle className="spin" size={17}/> : <FileImage size={17}/>} {isCreating ? t("creatingPdf") : t("createPdf")}</button></div>
  </>;
}

type PdfImagesPageMode = "all" | "current" | "custom";
type PdfImagesFormat = "jpg" | "png" | "webp";
type PdfImagesResolution = "72" | "150" | "300" | "custom";
type PdfImagesPreview = { count: number; errorKey?: TranslationKey };

function validatePdfImagePages(value: string, totalPages: number): PdfImagesPreview {
  if (!value.trim()) return { count: 0, errorKey: "pdfImagesPagesRequired" };
  const selected = new Set<number>();
  for (const rawToken of value.split(",")) {
    const token = rawToken.trim();
    const match = token.match(/^(\d+)(?:\s*-\s*(\d+))?$/);
    if (!match) return { count: 0, errorKey: "pdfImagesInvalidPages" };
    const start = Number(match[1]);
    const end = Number(match[2] ?? match[1]);
    if (start < 1) return { count: 0, errorKey: "pdfImagesInvalidPages" };
    if (end < start) return { count: 0, errorKey: "pdfImagesInvalidRange" };
    if (end > totalPages) return { count: 0, errorKey: "pdfImagesPageOutOfRange" };
    for (let page = start; page <= end; page += 1) selected.add(page);
  }
  return { count: selected.size };
}

function PdfToImagesPanel() {
  const { t } = useI18n();
  const { settings } = useSettings();
  const [file, setFile] = useState<PdfItem | null>(null);
  const [destination, setDestination] = useState<string | null>(null);
  const [pageMode, setPageMode] = useState<PdfImagesPageMode>("all");
  const [currentPage, setCurrentPage] = useState("1");
  const [customPages, setCustomPages] = useState("");
  const [format, setFormat] = useState<PdfImagesFormat>("png");
  const [quality, setQuality] = useState(90);
  const [resolution, setResolution] = useState<PdfImagesResolution>("150");
  const [customDpi, setCustomDpi] = useState("200");
  const [transparent, setTransparent] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [openImageError, setOpenImageError] = useState<string | null>(null);
  const [previewImage, setPreviewImage] = useState<PdfImagesOutput | null>(null);
  const [password, setPassword] = useState("");
  const [isConverting, setIsConverting] = useState(false);
  const [progress, setProgress] = useState<PdfImagesProgress>({ completed: 0, total: 0, percent: 0, currentPage: 0, currentFile: "" });
  const [result, setResult] = useState<PdfImagesResult | null>(null);

  const addPath = useCallback(async (path: string) => {
    if (isConverting) return;
    setNotice(null); setOpenImageError(null); setResult(null); setPassword("");
    setFile({ path, name: nameFromPath(path), size: 0, pages: 0, status: "reading" });
    try {
      const info = await invoke<PdfInfo>("inspect_pdf_for_images", { path, password: null });
      setFile({ ...info, status: "ready" });
      setCurrentPage("1");
      setCustomPages("");
    } catch (error) {
      const parsed = parseError(error);
      setFile((current) => current?.path === path ? { ...current, status: "failed", errorCode: parsed.code, errorMessage: parsed.message } : current);
    }
  }, [isConverting]);

  useEffect(() => {
    let cleanup: (() => void) | undefined; let disposed = false;
    getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "drop" && event.payload.paths[0]) void addPath(event.payload.paths[0]);
    }).then((unlisten) => { if (disposed) unlisten(); else cleanup = unlisten; });
    return () => { disposed = true; cleanup?.(); };
  }, [addPath]);

  useEffect(() => {
    let cleanup: (() => void) | undefined; let disposed = false;
    listen<PdfImagesProgress>("pdf-images-progress", (event) => setProgress(event.payload))
      .then((unlisten) => { if (disposed) unlisten(); else cleanup = unlisten; });
    return () => { disposed = true; cleanup?.(); };
  }, []);

  useEffect(() => {
    if (!previewImage) return;
    const closePreview = (event: KeyboardEvent) => { if (event.key === "Escape") setPreviewImage(null); };
    window.addEventListener("keydown", closePreview);
    return () => window.removeEventListener("keydown", closePreview);
  }, [previewImage]);

  async function choosePdf() {
    const selected = await open({ multiple: false, directory: false, filters: [{ name: "PDF", extensions: ["pdf"] }] });
    if (typeof selected === "string") await addPath(selected);
  }
  async function chooseDestination() {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === "string") setDestination(selected);
  }

  async function unlockPdf() {
    if (!file || !password) { setNotice(t("pdfImagesPasswordRequired")); return; }
    setNotice(null); setFile((current) => current ? { ...current, status: "reading" } : current);
    try {
      const info = await invoke<PdfInfo>("inspect_pdf_for_images", { path: file.path, password });
      setFile({ ...info, status: "ready" });
    } catch (error) {
      const parsed = parseError(error);
      setFile((current) => current ? { ...current, status: "failed", errorCode: parsed.code, errorMessage: parsed.message } : current);
      setNotice(t(errorKeys[parsed.code ?? "unknown_error"] ?? "unknownError"));
    }
  }

  const preview = useMemo<PdfImagesPreview>(() => {
    if (!file || file.status !== "ready") return { count: 0 };
    if (pageMode === "all") return { count: file.pages };
    if (pageMode === "current") {
      if (!/^\d+$/.test(currentPage.trim()) || Number(currentPage) < 1) return { count: 0, errorKey: "pdfImagesInvalidCurrentPage" };
      if (Number(currentPage) > file.pages) return { count: 0, errorKey: "pdfImagesPageOutOfRange" };
      return { count: 1 };
    }
    return validatePdfImagePages(customPages, file.pages);
  }, [currentPage, customPages, file, pageMode]);

  const dpiError = resolution === "custom" && (!/^\d+$/.test(customDpi.trim()) || Number(customDpi) < 1);
  const fileError = file?.status === "failed" ? t(errorKeys[file.errorCode ?? "unknown_error"] ?? "unknownError") : null;
  const validationError = preview.errorKey ? t(preview.errorKey, { pages: file?.pages ?? 0 }) : dpiError ? t("pdfImagesInvalidDpi") : null;

  async function convertPdf() {
    if (!file || file.status !== "ready") { setNotice(t("pdfImagesNeedPdf")); return; }
    if (preview.errorKey) { setNotice(t(preview.errorKey, { pages: file.pages })); return; }
    if (dpiError) { setNotice(t("pdfImagesInvalidDpi")); return; }
    if ((format === "jpg" || format === "webp") && (quality < 10 || quality > 100)) { setNotice(t("pdfImagesInvalidQuality")); return; }
    if (isConverting) return;
    setIsConverting(true); setNotice(null); setOpenImageError(null); setResult(null);
    setProgress({ completed: 0, total: preview.count, percent: 0, currentPage: 0, currentFile: "" });
    const startTime = Date.now();
    try {
      const response = await invoke<PdfImagesResult>("pdf_to_images", { request: {
        path: file.path, destinationDir: destination, pageMode,
        currentPage: pageMode === "current" ? Number(currentPage) : null,
        customPages: pageMode === "custom" ? customPages : null,
        format, quality, dpi: resolution === "custom" ? Number(customDpi) : Number(resolution),
        transparentBackground: format === "png" && transparent, password: password || null,
      }});
      const durationMs = Date.now() - startTime;
      const historyEntries: ProcessingHistoryEntry[] = [
        ...response.files.map((output) => createPdfProcessingHistoryEntry({
          tool: "PDF to Images",
          sourceFileName: `${file.name} (page ${output.page})`,
          resultPath: output.path,
          success: true,
          originalSize: file.size,
          finalSize: output.size,
          durationMs,
        })),
        ...response.failures.map((failure) => createPdfProcessingHistoryEntry({
          tool: "PDF to Images",
          sourceFileName: `${file.name} (page ${failure.page})`,
          success: false,
          originalSize: file.size,
          durationMs,
          errorMessage: failure.message || failure.code,
        })),
      ];
      if (historyEntries.length === 0) {
        historyEntries.push(createPdfProcessingHistoryEntry({
          tool: "PDF to Images",
          sourceFileName: file.name,
          success: false,
          originalSize: file.size,
          durationMs,
          errorMessage: response.errorMessage ?? (response.errorCode ? t(errorKeys[response.errorCode] ?? "unknownError", { pages: file.pages }) : t("unknownError")),
        }));
      }
      void appendProcessingHistoryEntries(historyEntries, settings.enableProcessingHistory);
      setResult(response.files.length || response.failures.length ? response : null);
      if (response.success) setNotice(response.failedCount ? t("pdfImagesPartialComplete", { success: response.files.length, failed: response.failedCount }) : t("pdfImagesComplete", { count: response.files.length }));
      else setNotice(response.errorCode ? t(errorKeys[response.errorCode] ?? "unknownError", { pages: file.pages }) : response.errorMessage ?? t("unknownError"));
    } catch (error) {
      const parsed = parseError(error);
      void appendProcessingHistoryEntry(createPdfProcessingHistoryEntry({
        tool: "PDF to Images",
        sourceFileName: file.name,
        success: false,
        originalSize: file.size,
        durationMs: Date.now() - startTime,
        errorMessage: parsed.message ?? (parsed.code ? t(errorKeys[parsed.code] ?? "unknownError", { pages: file.pages }) : t("unknownError")),
      }), settings.enableProcessingHistory);
      setNotice(parsed.code ? t(errorKeys[parsed.code] ?? "unknownError", { pages: file.pages }) : parsed.message ?? t("unknownError"));
    } finally { setIsConverting(false); }
  }

  async function validateGeneratedImage(output: PdfImagesOutput) {
    return invoke<PdfImageOpenValidation>("validate_pdf_image_output", { path: output.path });
  }

  async function previewGeneratedImage(output: PdfImagesOutput) {
    setOpenImageError(null);
    try {
      const validation = await validateGeneratedImage(output);
      setPreviewImage({ ...output, path: validation.path });
    } catch (error) {
      const parsed = parseError(error);
      const detail = parsed.code && parsed.code !== "unknown_error" && errorKeys[parsed.code]
        ? t(errorKeys[parsed.code])
        : t("previewImageFailed");
      setOpenImageError(t("openImageFailedWithName", { name: output.name, detail }));
    }
  }

  async function openGeneratedImage(output: PdfImagesOutput) {
    setOpenImageError(null);
    try {
      const validation = await validateGeneratedImage(output);
      try {
        await openPath(validation.path);
      } catch {
        try {
          await openPath(validation.path, "mspaint");
        } catch {
          try { await revealItemInDir(validation.path); } catch { /* The visible message below remains authoritative. */ }
          setOpenImageError(t("openImageNoWindowsApp", { name: output.name }));
        }
      }
    } catch (error) {
      const parsed = parseError(error);
      const detail = parsed.code && parsed.code !== "unknown_error" && errorKeys[parsed.code]
        ? t(errorKeys[parsed.code])
        : t("openImageFailed");
      setOpenImageError(t("openImageFailedWithName", { name: output.name, detail }));
    }
  }

  async function showGeneratedImage(output: PdfImagesOutput) {
    setOpenImageError(null);
    try {
      const validation = await validateGeneratedImage(output);
      await revealItemInDir(validation.path);
    } catch (error) {
      const parsed = parseError(error);
      const detail = parsed.code && parsed.code !== "unknown_error" && errorKeys[parsed.code]
        ? t(errorKeys[parsed.code])
        : t("showInFolderFailed");
      setOpenImageError(t("openImageFailedWithName", { name: output.name, detail }));
    }
  }

  const pageModes: { id: PdfImagesPageMode; key: TranslationKey; hint: TranslationKey }[] = [
    { id: "all", key: "allPages", hint: "allPagesHint" },
    { id: "current", key: "currentPage", hint: "currentPageHint" },
    { id: "custom", key: "customPages", hint: "customPagesHint" },
  ];
  const resolutions: { id: PdfImagesResolution; label: string }[] = [
    { id: "72", label: "72 DPI" }, { id: "150", label: "150 DPI" },
    { id: "300", label: "300 DPI" }, { id: "custom", label: t("custom") },
  ];

  return <>
    <section className="pdf-drop-zone pdf-images-drop" onClick={() => !isConverting && void choosePdf()}><div className="compress-drop-icon"><UploadCloud size={28}/></div><div><h2>{t("pdfImagesDropTitle")}</h2><p>{t("pdfImagesDropSubtitle")}</p></div><button className="primary-button" disabled={isConverting} onClick={(event) => { event.stopPropagation(); void choosePdf(); }}><FilePlus2 size={17}/>{file ? t("replacePdf") : t("addPdf")}</button></section>
    <section className="compress-panel pdf-images-file-panel"><div className="compress-panel-header"><div><h2>{t("selectedPdf")}</h2><span>{t("singlePdfOnly")}</span></div></div>{!file ? <div className="empty-image-list"><Files size={25}/><strong>{t("noPdfSelected")}</strong><p>{t("pdfImagesNeedPdf")}</p></div> : <><div className={`split-file-row ${file.status}`}><Files size={24}/><span className="pdf-name" title={file.path}>{file.name}{fileError && <small>{fileError}</small>}</span><span><small>{t("fileSize")}</small><strong>{bytes(file.size)}</strong></span><span><small>{t("totalPages")}</small><strong>{file.pages || "—"}</strong></span><button className="secondary-button" disabled={isConverting} onClick={() => void choosePdf()}>{t("replacePdf")}</button><button className="icon-button remove-button" aria-label={t("removeFile")} disabled={isConverting} onClick={() => { setFile(null); setNotice(null); setResult(null); setPassword(""); }}><Trash2 size={14}/></button></div>{file.status === "failed" && file.errorCode === "pdf_images_password_invalid" && <div className="pdf-images-password"><div><strong>{t("passwordProtectedPdf")}</strong><small>{t("pdfImagesPasswordHint")}</small></div><label><input type="password" autoComplete="off" value={password} placeholder={t("pdfPassword")} onChange={(event) => setPassword(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") void unlockPdf(); }}/><button className="secondary-button" disabled={!password} onClick={() => void unlockPdf()}>{t("unlockPdf")}</button></label></div>}</>}</section>
    <section className="pdf-images-settings">
      <div className="compress-panel pdf-images-pages"><div><h2>{t("pagesToConvert")}</h2><p>{t("pagesToConvertHint")}</p></div><div className="pdf-images-choice-grid">{pageModes.map((option) => <button key={option.id} className={pageMode === option.id ? "active" : ""} onClick={() => { setPageMode(option.id); setNotice(null); }}><strong>{t(option.key)}</strong><small>{t(option.hint)}</small></button>)}</div>{pageMode === "current" && <label className="pdf-images-number-field"><span>{t("currentPageNumber")}</span><input type="number" min="1" max={file?.pages} value={currentPage} onChange={(event) => { setCurrentPage(event.target.value); setNotice(null); }}/></label>}{pageMode === "custom" && <label className="pdf-images-custom-pages"><span>{t("customPages")}</span><input dir="ltr" value={customPages} placeholder="1, 3, 5-8" onChange={(event) => { setCustomPages(event.target.value); setNotice(null); }}/><small>{t("customPagesInputHint")}</small></label>}</div>
      <div className="compress-panel pdf-images-output-format"><div><h2>{t("outputFormat")}</h2><p>{t("pdfImagesFormatHint")}</p></div><div className="pdf-images-format-buttons">{(["jpg", "png", "webp"] as PdfImagesFormat[]).map((option) => <button key={option} className={format === option ? "active" : ""} onClick={() => { setFormat(option); if (option !== "png") setTransparent(false); setNotice(null); }}>{option.toUpperCase()}</button>)}</div>{format !== "png" && <div className="pdf-images-quality"><div><strong>{t("quality")}</strong><b>{quality}</b></div><input type="range" min="10" max="100" value={quality} onChange={(event) => { setQuality(Number(event.target.value)); setNotice(null); }}/><small>{t("pdfImagesQualityHint")}</small></div>}<label className={`pdf-images-transparent ${format !== "png" ? "disabled" : ""}`}><input type="checkbox" checked={transparent} disabled={format !== "png"} onChange={(event) => setTransparent(event.target.checked)}/><span><strong>{t("transparentBackground")}</strong><small>{format === "png" ? t("transparentBackgroundHint") : t("transparentPngOnly")}</small></span></label></div>
    </section>
    <section className="compress-panel pdf-images-resolution"><div><h2>{t("resolution")}</h2><p>{t("resolutionHint")}</p></div><div className="pdf-images-resolution-buttons">{resolutions.map((option) => <button key={option.id} className={resolution === option.id ? "active" : ""} onClick={() => { setResolution(option.id); setNotice(null); }}>{option.label}</button>)}</div>{resolution === "custom" && <label><span>{t("customDpi")}</span><div><input type="number" min="1" value={customDpi} onChange={(event) => { setCustomDpi(event.target.value); setNotice(null); }}/><b>DPI</b></div></label>}</section>
    <section className="compress-panel pdf-images-destination"><div><h2>{t("destinationFolder")}</h2><p>{t("pdfImagesDestinationHint")}</p></div><div className="pdf-destination"><MapPin size={17}/><span><small>{t("destinationFolder")}</small><strong title={destination ?? ""}>{destination ?? t("sameFolderAsPdf")}</strong></span><button className="secondary-button" onClick={() => void chooseDestination()}><FolderOpen size={15}/>{t("chooseFolder")}</button></div></section>
    <section className={`compress-panel pdf-images-preview ${validationError ? "invalid" : ""}`}><div className="pdf-images-preview-icon"><Layers3 size={24}/></div><div><h2>{t("conversionPreview")}</h2><p>{validationError ?? t("pdfImagesWillCreate", { count: preview.count })}</p></div><strong>{preview.count}</strong><span>{t("images")}</span></section>
    {isConverting && <section className="compress-panel split-progress pdf-images-progress"><div><span><strong>{t("renderingPdfPages")}</strong><small>{t("renderingPage", { page: progress.currentPage || "—" })}: <b dir="ltr">{progress.currentFile}</b></small></span><span>{progress.completed}/{progress.total} · {progress.percent}%</span></div><progress max="100" value={progress.percent}/></section>}
    {result && <section className="compress-panel pdf-images-results"><div className="pdf-images-results-header"><CheckCircle2 size={24}/><div><h2>{result.failedCount ? t("pdfImagesPartialComplete", { success: result.files.length, failed: result.failedCount }) : t("pdfImagesComplete", { count: result.files.length })}</h2><small title={result.destinationDir}>{result.destinationDir}</small></div><button className="primary-button" disabled={!result.destinationDir} onClick={() => result.destinationDir && void openPath(result.destinationDir)}><FolderOpen size={15}/>{t("openFolder")}</button></div><div className="pdf-images-result-summary"><span><small>{t("pagesConverted")}</small><strong>{result.totalPages}</strong></span><span><small>{t("failedPages")}</small><strong>{result.failedCount}</strong></span><span><small>{t("outputFormat")}</small><strong>{result.format}</strong></span><span><small>{t("resolutionUsed")}</small><strong>{result.dpi} DPI</strong></span><span><small>{t("totalImagesSize")}</small><strong>{bytes(result.totalSize)}</strong></span><span><small>{t("destinationFolder")}</small><strong title={result.destinationDir}>{result.destinationDir}</strong></span></div><div className="pdf-images-result-list">{result.files.map((output) => <div key={output.path}><div className="pdf-images-output-thumb"><FileImage size={16}/><img src={convertFileSrc(output.path)} alt="" onError={(event) => { event.currentTarget.style.display = "none"; }}/></div><span title={output.path}><strong>{output.name}</strong><small>{t("pageNumber", { page: output.page })} · {output.width} × {output.height} · {bytes(output.size)} · {output.format}</small></span><button className="text-button" onClick={() => void previewGeneratedImage(output)}>{t("previewImage")}</button><button className="text-button" onClick={() => void openGeneratedImage(output)}>{t("openImage")}</button><button className="text-button" onClick={() => void showGeneratedImage(output)}>{t("showInFolder")}</button></div>)}{result.failures.map((failure) => <div className="pdf-images-failed-row" key={`${failure.page}-${failure.code}`}><Trash2 size={15}/><span><strong>{t("failedPage", { page: failure.page })}</strong><small>{t(errorKeys[failure.code] ?? "pdfImagesRenderError")}</small></span></div>)}</div></section>}
    {previewImage && <div className="pdf-image-preview-backdrop" role="presentation" onMouseDown={(event) => { if (event.currentTarget === event.target) setPreviewImage(null); }}><section className="pdf-image-preview-modal" role="dialog" aria-modal="true" aria-label={t("previewImage")}><header><div><strong>{previewImage.name}</strong><small>{previewImage.width} × {previewImage.height} · {bytes(previewImage.size)} · {previewImage.format}</small></div><button className="icon-button" aria-label={t("closePreview")} onClick={() => setPreviewImage(null)}><X size={17}/></button></header><div><img src={convertFileSrc(previewImage.path)} alt={previewImage.name} onError={() => { setPreviewImage(null); setOpenImageError(t("previewImageFailed")); }}/></div></section></div>}
    {openImageError && <div className="page-error">{openImageError}</div>}
    {notice && <div className={result?.success ? "split-info" : "page-error"}>{notice}</div>}
    <div className="pdf-main-action"><button className="primary-button" disabled={file?.status !== "ready" || isConverting} onClick={() => void convertPdf()}>{isConverting ? <LoaderCircle className="spin" size={17}/> : <FileImage size={17}/>} {isConverting ? t("renderingPdfPages") : t("convertPdfToImages")}</button></div>
  </>;
}

export function PdfToolsPage() {
  const { t, language, setLanguage } = useI18n();
  const { settings } = useSettings();
  const [tab, setTab] = useState<Tab>("merge");
  const [items, setItems] = useState<PdfItem[]>([]);
  const [destination, setDestination] = useState<string | null>(null);
  const [outputName, setOutputName] = useState("merged-document");
  const [isMerging, setIsMerging] = useState(false);
  const [progress, setProgress] = useState(0);
  const [result, setResult] = useState<MergeResult | null>(null);
  const [pageError, setPageError] = useState<string | null>(null);
  const dragging = useRef<string | null>(null);

  const addPaths = useCallback(async (paths: string[]) => {
    const accepted = paths.filter((path, index) => paths.indexOf(path) === index && !items.some((item) => item.path === path));
    if (!accepted.length) return;
    setResult(null); setPageError(null);
    setItems((current) => [...current, ...accepted.map((path) => ({ path, name: nameFromPath(path), size: 0, pages: 0, status: "reading" as const }))]);
    await Promise.all(accepted.map(async (path) => {
      try {
        const info = await invoke<PdfInfo>("inspect_pdf", { path });
        setItems((current) => current.map((item) => item.path === path ? { ...info, status: "ready" } : item));
      } catch (error) {
        const parsed = parseError(error);
        setItems((current) => current.map((item) => item.path === path ? { ...item, status: "failed", errorCode: parsed.code, errorMessage: parsed.message } : item));
      }
    }));
  }, [items]);

  useEffect(() => {
    let cleanup: (() => void) | undefined; let disposed = false;
    getCurrentWebview().onDragDropEvent((event) => { if (event.payload.type === "drop" && tab === "merge") void addPaths(event.payload.paths); })
      .then((unlisten) => { if (disposed) unlisten(); else cleanup = unlisten; });
    return () => { disposed = true; cleanup?.(); };
  }, [addPaths, tab]);

  async function choosePdfs() {
    const selected = await open({ multiple: true, directory: false, filters: [{ name: "PDF", extensions: ["pdf"] }] });
    if (selected) await addPaths(Array.isArray(selected) ? selected : [selected]);
  }
  async function chooseDestination() {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === "string") setDestination(selected);
  }
  function move(index: number, offset: number) {
    const target = index + offset; if (target < 0 || target >= items.length) return;
    setItems((current) => { const next = [...current]; [next[index], next[target]] = [next[target], next[index]]; return next; });
  }
  function dropOn(path: string) {
    const source = dragging.current; dragging.current = null; if (!source || source === path) return;
    setItems((current) => { const from = current.findIndex((item) => item.path === source); const to = current.findIndex((item) => item.path === path); const next = [...current]; const [entry] = next.splice(from, 1); next.splice(to, 0, entry); return next; });
  }
  function message(code?: string, fallback?: string) { return code ? t(errorKeys[code] ?? "unknownError") : fallback; }
  async function merge() {
    const valid = items.filter((item) => item.status === "ready");
    if (valid.length < 2) { setPageError(t("pdfNeedTwo")); return; }
    if (isMerging) return;
    setIsMerging(true); setProgress(8); setPageError(null); setResult(null);
    const sourceFileName = joinSourceNames(valid.map((item) => item.name));
    const originalSize = valid.reduce((total, item) => total + item.size, 0);
    const startTime = Date.now();
    const timer = window.setInterval(() => setProgress((value) => Math.min(value + Math.max(1, Math.round((92 - value) / 8)), 92)), 180);
    try {
      const response = await invoke<MergeResult>("merge_pdfs", { request: { paths: valid.map((item) => item.path), destinationDir: destination, outputName } });
      const durationMs = Date.now() - startTime;
      void appendProcessingHistoryEntry(createPdfProcessingHistoryEntry({
        tool: "Merge PDFs",
        sourceFileName,
        resultPath: response.outputPath,
        success: response.success,
        originalSize,
        finalSize: response.newSize,
        durationMs,
        errorMessage: response.success ? undefined : response.errorMessage ?? (response.errorCode ? message(response.errorCode, response.errorMessage) : t("unknownError")),
      }), settings.enableProcessingHistory);
      setProgress(100); setResult(response);
      if (!response.success) setPageError(message(response.errorCode, response.errorMessage) ?? t("unknownError"));
    } catch (error) {
      const parsed = parseError(error);
      void appendProcessingHistoryEntry(createPdfProcessingHistoryEntry({
        tool: "Merge PDFs",
        sourceFileName,
        success: false,
        originalSize,
        durationMs: Date.now() - startTime,
        errorMessage: parsed.message ?? message(parsed.code, parsed.message) ?? t("unknownError"),
      }), settings.enableProcessingHistory);
      setPageError(message(parsed.code, parsed.message) ?? t("unknownError"));
    } finally { window.clearInterval(timer); setIsMerging(false); }
  }

  return <div className="page pdf-page">
    <header className="page-header compression-header">
      <div><span className="eyebrow">{t("pdfModuleLabel")}</span><h1>{t("pdfToolsTitle")}</h1><p>{t("pdfToolsSubtitle")}</p></div>
      <div className="compress-header-actions"><label className="language-select"><span>{language.toUpperCase()}</span><select value={language} onChange={(event) => setLanguage(event.target.value as "en" | "fr" | "ar")} disabled={isMerging}><option value="en">English</option><option value="fr">Français</option><option value="ar">العربية</option></select></label><div className="header-status"><ShieldCheck size={17}/><span>{t("offline")}</span></div></div>
    </header>
    <nav className="pdf-tabs" aria-label={t("pdfToolsTitle")}>{tabs.map((entry) => <button key={entry.id} className={tab === entry.id ? "active" : ""} onClick={() => setTab(entry.id)}>{t(entry.key)}</button>)}</nav>
    {tab === "split" ? <SplitPdfPanel/> : tab === "organize" ? <OrganizePagesPanel/> : tab === "imagesToPdf" ? <ImagesToPdfPanel/> : tab === "pdfToImages" ? <PdfToImagesPanel/> : tab !== "merge" ? <section className="compress-panel pdf-coming-soon"><Files size={30}/><h2>{t(tabs.find((entry) => entry.id === tab)!.key)}</h2><p>{t("comingSoon")}</p></section> : <>
      <section className="pdf-drop-zone" onClick={() => !isMerging && void choosePdfs()}><div className="compress-drop-icon"><UploadCloud size={28}/></div><div><h2>{t("pdfDropTitle")}</h2><p>{t("pdfDropSubtitle")}</p></div><button className="primary-button" disabled={isMerging} onClick={(event) => { event.stopPropagation(); void choosePdfs(); }}><FilePlus2 size={17}/>{t("addPdfs")}</button></section>
      <section className="compress-panel file-panel pdf-file-panel"><div className="compress-panel-header"><div><h2>{t("selectedPdfs")}</h2><span>{t("pdfCount", { count: items.length })}</span></div>{items.length > 0 && <button className="text-button danger" disabled={isMerging} onClick={() => { setItems([]); setResult(null); }}><Trash2 size={14}/>{t("clearAll")}</button>}</div>
        {!items.length ? <div className="empty-image-list"><Files size={25}/><strong>{t("noPdfs")}</strong><p>{t("noPdfsHint")}</p></div> : <div className="pdf-list"><div className="pdf-list-head"><span></span><span>{t("fileName")}</span><span>{t("fileSize")}</span><span>{t("pages")}</span><span>{t("status")}</span><span>{t("order")}</span><span></span></div>{items.map((item, index) => <div className={`pdf-row ${item.status}`} key={item.path} draggable={!isMerging} onDragStart={() => { dragging.current = item.path; }} onDragOver={(event) => event.preventDefault()} onDrop={() => dropOn(item.path)}><GripVertical size={15}/><span className="pdf-name" title={item.path}>{item.name}{item.status === "failed" && <small>{message(item.errorCode, item.errorMessage)}</small>}</span><span>{bytes(item.size)}</span><span>{item.pages || "—"}</span><span className="status-pill">{t(item.status)}</span><span className="order-buttons"><button aria-label={t("moveUp")} disabled={isMerging || index === 0} onClick={() => move(index, -1)}><ArrowUp size={13}/></button><button aria-label={t("moveDown")} disabled={isMerging || index === items.length - 1} onClick={() => move(index, 1)}><ArrowDown size={13}/></button></span><button className="icon-button remove-button" aria-label={t("removeFile")} disabled={isMerging} onClick={() => setItems((current) => current.filter((entry) => entry.path !== item.path))}><Trash2 size={14}/></button></div>)}</div>}
      </section>
      <section className="compress-panel pdf-output-panel"><div><h2>{t("pdfOutputSettings")}</h2><p>{t("pdfOutputHint")}</p></div><label><span>{t("finalFileName")}</span><div><input value={outputName} onChange={(event) => setOutputName(event.target.value)} disabled={isMerging}/><b>.pdf</b></div></label><div className="pdf-destination"><MapPin size={17}/><span><small>{t("destinationFolder")}</small><strong title={destination ?? ""}>{destination ?? t("sameFolder")}</strong></span><button className="secondary-button" disabled={isMerging} onClick={() => void chooseDestination()}><FolderOpen size={15}/>{t("chooseFolder")}</button></div></section>
      {isMerging && <section className="compress-panel pdf-progress"><div><span>{t("mergingPdfs")}</span><strong>{progress}%</strong></div><progress max="100" value={progress}/></section>}
      {pageError && <div className="page-error">{pageError}</div>}
      {result?.success && result.outputPath && <section className="compress-panel pdf-result"><CheckCircle2 size={28}/><div><h2>{t("mergeComplete")}</h2><div className="pdf-result-grid"><span><small>{t("pdfsMerged")}</small><strong>{result.pdfCount}</strong></span><span><small>{t("totalPages")}</small><strong>{result.totalPages}</strong></span><span><small>{t("createdFileSize")}</small><strong>{bytes(result.newSize ?? 0)}</strong></span><span className="result-path"><small>{t("destinationPath")}</small><strong>{result.outputPath}</strong></span></div></div><div className="pdf-result-actions"><button className="secondary-button" onClick={() => void openPath(result.outputPath!)}>{t("openFile")}</button><button className="primary-button" onClick={() => void revealItemInDir(result.outputPath!)}><FolderOpen size={15}/>{t("openFolder")}</button></div></section>}
      <div className="pdf-main-action"><button className="primary-button" disabled={isMerging || items.filter((item) => item.status === "ready").length < 2} onClick={() => void merge()}>{isMerging ? <LoaderCircle className="spin" size={17}/> : <Files size={17}/>} {isMerging ? t("mergingPdfs") : t("mergePdfs")}</button></div>
    </>}
  </div>;
}
