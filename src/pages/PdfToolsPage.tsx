import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { ArrowDown, ArrowUp, CheckCircle2, FilePlus2, Files, FolderOpen, GripVertical, Layers3, ListOrdered, LoaderCircle, MapPin, Scissors, ShieldCheck, Trash2, UploadCloud } from "lucide-react";
import { useI18n, type TranslationKey } from "../i18n";

type Tab = "merge" | "split" | "organize" | "imagesToPdf" | "pdfToImages";
type ItemStatus = "reading" | "ready" | "failed";
interface PdfInfo { path: string; name: string; size: number; pages: number; }
interface PdfItem extends PdfInfo { status: ItemStatus; errorCode?: string; errorMessage?: string; }
interface MergeResult { success: boolean; pdfCount: number; totalPages: number; newSize?: number; outputPath?: string; errorCode?: string; errorMessage?: string; }
interface SplitOutput { path: string; name: string; pages: number; size: number; }
interface SplitResult { success: boolean; files: SplitOutput[]; totalPages: number; destinationDir?: string; errorCode?: string; errorMessage?: string; }
interface SplitProgress { completed: number; total: number; percent: number; currentFile: string; }

const tabs: { id: Tab; key: TranslationKey }[] = [
  { id: "merge", key: "mergePdfs" }, { id: "split", key: "splitPdf" },
  { id: "organize", key: "organizePages" }, { id: "imagesToPdf", key: "imagesToPdf" },
  { id: "pdfToImages", key: "pdfToImages" },
];
const errorKeys: Record<string, TranslationKey> = {
  corrupted_pdf: "pdfCorrupted", not_pdf: "pdfNotPdf", password_protected_pdf: "pdfPasswordProtected",
  destination_inaccessible: "pdfDestinationInaccessible", read_error: "pdfReadError", write_error: "pdfWriteError",
  insufficient_space: "pdfInsufficientSpace", not_enough_pdfs: "pdfNeedTwo", unknown_error: "unknownError",
  split_invalid_pages: "splitInvalidPages", split_page_zero: "splitPageZero", split_range_reversed: "splitRangeReversed",
  split_page_out_of_range: "splitPageOutOfRange", split_invalid_ranges: "splitInvalidRanges",
  split_invalid_n: "splitInvalidN", split_invalid_mode: "splitInvalidMode", split_already_running: "splitAlreadyRunning",
};
function bytes(value: number) {
  if (!value) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const unit = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  const amount = value / 1024 ** unit;
  return `${amount >= 10 || unit === 0 ? amount.toFixed(0) : amount.toFixed(1)} ${units[unit]}`;
}
function nameFromPath(path: string) { return path.split(/[\\/]/).pop() || path; }
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
    try {
      const response = await invoke<SplitResult>("split_pdf", { request: {
        path: file.path, destinationDir: destination, mode,
        pages: mode === "extract" ? extractPages : null,
        ranges: mode === "ranges" ? ranges : null,
        everyN: mode === "everyN" ? Number(everyN) : null,
      }});
      if (response.success) {
        setSplitResult(response);
        setSplitProgress({ completed: response.files.length, total: response.files.length, percent: 100, currentFile: "" });
        setNotice(t("splitComplete", { count: response.files.length }));
      } else {
        setNotice(response.errorCode ? t(errorKeys[response.errorCode] ?? "unknownError", { pages: file.pages }) : response.errorMessage ?? t("unknownError"));
      }
    } catch (error) {
      const parsed = parseError(error);
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

export function PdfToolsPage() {
  const { t, language, setLanguage } = useI18n();
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
    const timer = window.setInterval(() => setProgress((value) => Math.min(value + Math.max(1, Math.round((92 - value) / 8)), 92)), 180);
    try {
      const response = await invoke<MergeResult>("merge_pdfs", { request: { paths: valid.map((item) => item.path), destinationDir: destination, outputName } });
      setProgress(100); setResult(response);
      if (!response.success) setPageError(message(response.errorCode, response.errorMessage) ?? t("unknownError"));
    } catch (error) {
      const parsed = parseError(error); setPageError(message(parsed.code, parsed.message) ?? t("unknownError"));
    } finally { window.clearInterval(timer); setIsMerging(false); }
  }

  return <div className="page pdf-page">
    <header className="page-header compression-header">
      <div><span className="eyebrow">{t("pdfModuleLabel")}</span><h1>{t("pdfToolsTitle")}</h1><p>{t("pdfToolsSubtitle")}</p></div>
      <div className="compress-header-actions"><label className="language-select"><span>{language.toUpperCase()}</span><select value={language} onChange={(event) => setLanguage(event.target.value as "en" | "fr" | "ar")} disabled={isMerging}><option value="en">English</option><option value="fr">Français</option><option value="ar">العربية</option></select></label><div className="header-status"><ShieldCheck size={17}/><span>{t("offline")}</span></div></div>
    </header>
    <nav className="pdf-tabs" aria-label={t("pdfToolsTitle")}>{tabs.map((entry) => <button key={entry.id} className={tab === entry.id ? "active" : ""} onClick={() => setTab(entry.id)}>{t(entry.key)}</button>)}</nav>
    {tab === "split" ? <SplitPdfPanel/> : tab !== "merge" ? <section className="compress-panel pdf-coming-soon"><Files size={30}/><h2>{t(tabs.find((entry) => entry.id === tab)!.key)}</h2><p>{t("comingSoon")}</p></section> : <>
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
