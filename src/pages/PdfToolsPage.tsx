import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { ArrowDown, ArrowUp, CheckCircle2, FilePlus2, Files, FolderOpen, GripVertical, LoaderCircle, MapPin, ShieldCheck, Trash2, UploadCloud } from "lucide-react";
import { useI18n, type TranslationKey } from "../i18n";

type Tab = "merge" | "split" | "organize" | "imagesToPdf" | "pdfToImages";
type ItemStatus = "reading" | "ready" | "failed";
interface PdfInfo { path: string; name: string; size: number; pages: number; }
interface PdfItem extends PdfInfo { status: ItemStatus; errorCode?: string; errorMessage?: string; }
interface MergeResult { success: boolean; pdfCount: number; totalPages: number; newSize?: number; outputPath?: string; errorCode?: string; errorMessage?: string; }

const tabs: { id: Tab; key: TranslationKey }[] = [
  { id: "merge", key: "mergePdfs" }, { id: "split", key: "splitPdf" },
  { id: "organize", key: "organizePages" }, { id: "imagesToPdf", key: "imagesToPdf" },
  { id: "pdfToImages", key: "pdfToImages" },
];
const errorKeys: Record<string, TranslationKey> = {
  corrupted_pdf: "pdfCorrupted", not_pdf: "pdfNotPdf", password_protected_pdf: "pdfPasswordProtected",
  destination_inaccessible: "pdfDestinationInaccessible", read_error: "pdfReadError", write_error: "pdfWriteError",
  insufficient_space: "pdfInsufficientSpace", not_enough_pdfs: "pdfNeedTwo", unknown_error: "unknownError",
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
    {tab !== "merge" ? <section className="compress-panel pdf-coming-soon"><Files size={30}/><h2>{t(tabs.find((entry) => entry.id === tab)!.key)}</h2><p>{t("comingSoon")}</p></section> : <>
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
