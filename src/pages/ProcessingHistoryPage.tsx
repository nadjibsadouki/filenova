import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  ExternalLink,
  FolderOpen,
  RefreshCw,
  Search,
  SlidersHorizontal,
  Trash2,
  XCircle,
} from "lucide-react";
import { useI18n, type TranslationKey } from "../i18n";
import {
  clearProcessingHistoryWithResult,
  loadProcessingHistoryWithResult,
  PROCESSING_HISTORY_CHANGED_EVENT,
  removeProcessingHistoryEntry,
  type ProcessingHistoryChangedDetail,
  type ProcessingHistoryEntry,
  type ProcessingHistoryStatus,
} from "../history";
import {
  canOpenProcessingResult,
  DEFAULT_PROCESSING_HISTORY_FILTERS,
  filterProcessingHistory,
  formatHistoryBytes,
  formatHistoryDate,
  formatHistoryDuration,
  type ProcessingHistoryCategoryFilter,
  type ProcessingHistoryStatusFilter,
  type ProcessingResultAvailability,
} from "../historyView";

const categories: Array<{ key: ProcessingHistoryCategoryFilter; labelKey: TranslationKey }> = [
  { key: "all", labelKey: "historyFilterAll" },
  { key: "Image", labelKey: "historyFilterImages" },
  { key: "PDF", labelKey: "historyFilterPdfs" },
  { key: "Video", labelKey: "historyFilterVideos" },
];

const statuses: Array<{ key: ProcessingHistoryStatusFilter; labelKey: TranslationKey }> = [
  { key: "all", labelKey: "historyStatusAll" },
  { key: "Completed", labelKey: "historyStatusCompleted" },
  { key: "Failed", labelKey: "historyStatusFailed" },
  { key: "Cancelled", labelKey: "historyStatusCancelled" },
];

const statusClasses: Record<ProcessingHistoryStatus, string> = {
  Completed: "badge-completed",
  Failed: "badge-failed",
  Cancelled: "badge-cancelled",
};

const statusLabels: Record<ProcessingHistoryStatus, TranslationKey> = {
  Completed: "historyStatusCompleted",
  Failed: "historyStatusFailed",
  Cancelled: "historyStatusCancelled",
};

function sortRecentFirst(entries: ProcessingHistoryEntry[]) {
  return [...entries].sort((a, b) => b.datetime.localeCompare(a.datetime));
}

export function ProcessingHistoryPage() {
  const { t, language } = useI18n();
  const [entries, setEntries] = useState<ProcessingHistoryEntry[]>([]);
  const [categoryFilter, setCategoryFilter] = useState<ProcessingHistoryCategoryFilter>("all");
  const [statusFilter, setStatusFilter] = useState<ProcessingHistoryStatusFilter>("all");
  const [search, setSearch] = useState("");
  const [availability, setAvailability] = useState<Record<string, ProcessingResultAvailability>>({});
  const [notice, setNotice] = useState<{ kind: "success" | "error" | "info"; text: string } | null>(null);
  const availabilityRequest = useRef(0);

  async function checkAvailability(entry: ProcessingHistoryEntry): Promise<ProcessingResultAvailability> {
    if (entry.status !== "Completed" || !entry.createdFilePath.trim()) {
      return { fileExists: false, parentExists: false, parentPath: null };
    }
    try {
      return await invoke<ProcessingResultAvailability>("check_processing_result_availability", {
        path: entry.createdFilePath,
      });
    } catch {
      return { fileExists: false, parentExists: false, parentPath: null };
    }
  }

  async function refreshAvailability(nextEntries: ProcessingHistoryEntry[]) {
    const request = ++availabilityRequest.current;
    const completedEntries = nextEntries.filter((entry) => (
      entry.status === "Completed" && entry.createdFilePath.trim().length > 0
    ));
    const checked = await Promise.all(completedEntries.map(async (entry) => (
      [entry.id, await checkAvailability(entry)] as const
    )));
    if (request === availabilityRequest.current) {
      setAvailability(Object.fromEntries(checked));
    }
  }

  async function refresh(showConfirmation = false) {
    const loaded = await loadProcessingHistoryWithResult();
    if (!loaded.success) {
      setNotice({ kind: "error", text: t("historyStorageError") });
      return;
    }
    const sorted = sortRecentFirst(loaded.entries);
    setEntries(sorted);
    await refreshAvailability(sorted);
    if (showConfirmation) setNotice({ kind: "success", text: t("historyRefreshed") });
  }

  useEffect(() => {
    const handleHistoryChanged = (event: Event) => {
      const changed = event as CustomEvent<ProcessingHistoryChangedDetail>;
      const sorted = sortRecentFirst(changed.detail?.entries ?? []);
      setEntries(sorted);
      void refreshAvailability(sorted);
    };

    void refresh();
    window.addEventListener(PROCESSING_HISTORY_CHANGED_EVENT, handleHistoryChanged);
    return () => window.removeEventListener(PROCESSING_HISTORY_CHANGED_EVENT, handleHistoryChanged);
  }, []);

  const filteredEntries = useMemo(() => filterProcessingHistory(entries, {
    category: categoryFilter,
    status: statusFilter,
    search,
  }), [entries, categoryFilter, statusFilter, search]);

  const filtersActive = categoryFilter !== DEFAULT_PROCESSING_HISTORY_FILTERS.category
    || statusFilter !== DEFAULT_PROCESSING_HISTORY_FILTERS.status
    || search !== DEFAULT_PROCESSING_HISTORY_FILTERS.search;

  function clearFilters() {
    setCategoryFilter(DEFAULT_PROCESSING_HISTORY_FILTERS.category);
    setStatusFilter(DEFAULT_PROCESSING_HISTORY_FILTERS.status);
    setSearch(DEFAULT_PROCESSING_HISTORY_FILTERS.search);
  }

  async function openResult(entry: ProcessingHistoryEntry) {
    const current = await checkAvailability(entry);
    setAvailability((values) => ({ ...values, [entry.id]: current }));
    if (!current.fileExists || !entry.createdFilePath.trim()) {
      setNotice({ kind: "error", text: t("historyFileUnavailable") });
      return;
    }
    try {
      await openPath(entry.createdFilePath);
      setNotice(null);
    } catch {
      setNotice({ kind: "error", text: t("historyOpenError") });
    }
  }

  async function openResultFolder(entry: ProcessingHistoryEntry) {
    const current = await checkAvailability(entry);
    setAvailability((values) => ({ ...values, [entry.id]: current }));
    try {
      if (current.fileExists) {
        try {
          await revealItemInDir(entry.createdFilePath);
          setNotice(null);
          return;
        } catch {
          // Fall back to opening the verified parent directory.
        }
      }
      if (current.parentExists && current.parentPath) {
        await openPath(current.parentPath);
        setNotice(current.fileExists ? null : { kind: "info", text: t("historyFileUnavailable") });
        return;
      }
      setNotice({
        kind: "error",
        text: current.fileExists ? t("historyOpenError") : t("historyFolderUnavailable"),
      });
    } catch {
      setNotice({ kind: "error", text: t("historyOpenError") });
    }
  }

  async function removeEntry(entry: ProcessingHistoryEntry) {
    const name = entry.createdFileName || entry.sourceFileName;
    if (!window.confirm(t("historyRemoveConfirm", { name }))) return;
    const result = await removeProcessingHistoryEntry(entry.id);
    if (!result.success) {
      setNotice({ kind: "error", text: t("historyRemoveError") });
      return;
    }
    setEntries(sortRecentFirst(result.entries));
    setAvailability((values) => {
      const updated = { ...values };
      delete updated[entry.id];
      return updated;
    });
    setNotice({ kind: "success", text: t("historyRemoved") });
  }

  async function clearHistory() {
    if (!entries.length || !window.confirm(t("historyClearConfirm", { count: entries.length }))) return;
    const result = await clearProcessingHistoryWithResult();
    if (!result.success) {
      setNotice({ kind: "error", text: t("historyStorageError") });
      return;
    }
    setEntries([]);
    setAvailability({});
    setNotice({ kind: "success", text: t("historyCleared") });
  }

  return (
    <div className="page history-page">
      <header className="page-header">
        <div>
          <span className="eyebrow">{t("historyEyebrow")}</span>
          <h1>{t("historyTitle")}</h1>
          <p>{t("historySubtitle")}</p>
        </div>
      </header>

      <section className="history-summary">
        <div>{t("historyTotalCount", { count: entries.length })}</div>
        <div className="history-summary-actions">
          <button className="text-button" type="button" onClick={() => void refresh(true)}>
            <RefreshCw size={16} />{t("historyRefresh")}
          </button>
          <button className="text-button danger" type="button" disabled={!entries.length} onClick={() => void clearHistory()}>
            <Trash2 size={16} />{t("historyClearButton")}
          </button>
        </div>
      </section>

      {notice && <div className={`history-notice ${notice.kind}`} role="status">{notice.text}</div>}

      <section className="history-filters">
        <div className="filter-group">
          {categories.map((option) => (
            <button
              key={option.key}
              type="button"
              className={categoryFilter === option.key ? "filter-button active" : "filter-button"}
              onClick={() => setCategoryFilter(option.key)}
            >
              {t(option.labelKey)}
            </button>
          ))}
        </div>
        <div className="filter-group">
          {statuses.map((option) => (
            <button
              key={option.key}
              type="button"
              className={statusFilter === option.key ? "filter-button active" : "filter-button"}
              onClick={() => setStatusFilter(option.key)}
            >
              {t(option.labelKey)}
            </button>
          ))}
        </div>
        <div className="history-search-row">
          <label className="history-search"><Search size={16} /><input value={search} placeholder={t("historySearchPlaceholder")} onChange={(event) => setSearch(event.target.value)} /></label>
          <button className="secondary-button" type="button" disabled={!filtersActive} onClick={clearFilters}>
            <SlidersHorizontal size={15} />{t("historyClearFilters")}
          </button>
        </div>
      </section>

      {filteredEntries.length === 0 ? (
        <section className="history-empty">
          <div className="empty-recent"><span>⌛</span><div><strong>{entries.length ? t("historyNoFilteredResults") : t("historyEmptyTitle")}</strong><p>{entries.length ? t("historyClearFilters") : t("historyEmptyHint")}</p></div></div>
        </section>
      ) : (
        <section className="history-list">
          {filteredEntries.map((entry) => {
            const resultAvailability = availability[entry.id];
            const completed = entry.status === "Completed";
            const resultMissing = completed && resultAvailability?.fileExists === false;
            return (
              <article className={`history-entry ${resultMissing ? "missing-result" : ""}`} key={entry.id}>
                <div className="history-entry-main">
                  <div className="history-entry-meta">
                    <strong>{entry.tool}</strong>
                    <span>{entry.category}</span>
                    <span>{formatHistoryDate(entry.datetime, language)}</span>
                  </div>
                  <div className="history-entry-files">
                    <div><small>{t("historySource")}</small><strong title={entry.sourceFileName}>{entry.sourceFileName}</strong></div>
                    <div><small>{t("historyResult")}</small><strong title={entry.createdFilePath}>{entry.createdFileName || "—"}</strong></div>
                  </div>
                  <div className="history-entry-data">
                    <span><small>{t("historyOriginalSize")}</small><strong>{formatHistoryBytes(entry.originalSize)}</strong></span>
                    <span><small>{t("historyFinalSize")}</small><strong>{formatHistoryBytes(entry.finalSize)}</strong></span>
                    <span><small>{t("historyDuration")}</small><strong>{formatHistoryDuration(entry.durationMs)}</strong></span>
                  </div>
                  {entry.details && <p className="history-entry-details">{entry.details}</p>}
                  {entry.errorMessage && <p className="history-entry-error"><XCircle size={14} /><span><strong>{t("historyError")}:</strong> {entry.errorMessage}</span></p>}
                  {resultMissing && <p className="history-entry-unavailable">{t("historyFileUnavailable")}</p>}
                </div>
                <aside className="history-entry-side">
                  <span className={`history-badge ${statusClasses[entry.status]}`}>{t(statusLabels[entry.status])}</span>
                  <div className="history-entry-actions">
                    <button type="button" className="text-button" disabled={!canOpenProcessingResult(entry, resultAvailability)} onClick={() => void openResult(entry)}>
                      <ExternalLink size={14} />{t("historyOpenFile")}
                    </button>
                    <button type="button" className="text-button" disabled={!completed || (!resultAvailability?.fileExists && !resultAvailability?.parentExists)} onClick={() => void openResultFolder(entry)}>
                      <FolderOpen size={14} />{t("historyOpenFolder")}
                    </button>
                    <button type="button" className="text-button danger" onClick={() => void removeEntry(entry)}>
                      <Trash2 size={14} />{t("historyRemoveEntry")}
                    </button>
                  </div>
                </aside>
              </article>
            );
          })}
        </section>
      )}
    </div>
  );
}
