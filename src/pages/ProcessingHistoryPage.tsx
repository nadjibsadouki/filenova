import { useEffect, useMemo, useState } from "react";
import { Search, Trash2 } from "lucide-react";
import { useI18n, type TranslationKey } from "../i18n";
import { loadProcessingHistory, type ProcessingHistoryCategory, type ProcessingHistoryEntry, type ProcessingHistoryStatus } from "../history";

const categories: Array<{ key: "all" | ProcessingHistoryCategory; labelKey: TranslationKey }> = [
  { key: "all", labelKey: "historyFilterAll" },
  { key: "Image", labelKey: "historyFilterImages" },
  { key: "PDF", labelKey: "historyFilterPdfs" },
  { key: "Video", labelKey: "historyFilterVideos" },
];

const statuses: Array<{ key: "all" | ProcessingHistoryStatus; labelKey: TranslationKey }> = [
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
  const { t } = useI18n();
  const [entries, setEntries] = useState<ProcessingHistoryEntry[]>([]);
  const [categoryFilter, setCategoryFilter] = useState<"all" | ProcessingHistoryCategory>("all");
  const [statusFilter, setStatusFilter] = useState<"all" | ProcessingHistoryStatus>("all");
  const [search, setSearch] = useState("");

  useEffect(() => {
    void (async () => {
      const loaded = await loadProcessingHistory();
      setEntries(sortRecentFirst(loaded));
    })();
  }, []);

  const filteredEntries = useMemo(() => {
    return entries.filter((entry) => {
      const categoryMatch = categoryFilter === "all" || entry.category === categoryFilter;
      const statusMatch = statusFilter === "all" || entry.status === statusFilter;
      const searchMatch = search.trim().length === 0 || [entry.sourceFileName, entry.createdFileName].some((value) => value.toLowerCase().includes(search.trim().toLowerCase()));
      return categoryMatch && statusMatch && searchMatch;
    });
  }, [entries, categoryFilter, statusFilter, search]);

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
        <button className="text-button" type="button"><Trash2 size={16} />{t("historyClearButton")}</button>
      </section>

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
        <label className="history-search"><Search size={16} /><input value={search} placeholder={t("historySearchPlaceholder")} onChange={(event) => setSearch(event.target.value)} /></label>
      </section>

      {filteredEntries.length === 0 ? (
        <section className="history-empty">
          <div className="empty-recent"><span>⌛</span><div><strong>{t("historyEmptyTitle")}</strong><p>{t("historyEmptyHint")}</p></div></div>
        </section>
      ) : (
        <section className="history-list">
          {filteredEntries.map((entry) => (
            <article className="history-entry" key={entry.id}>
              <div className="history-entry-main">
                <div className="history-entry-meta"><strong>{entry.tool}</strong><span>{new Date(entry.datetime).toLocaleString()}</span></div>
                <div className="history-entry-file"><strong>{entry.sourceFileName}</strong><span>{entry.createdFileName}</span></div>
              </div>
              <div className="history-entry-status"><span className={`history-badge ${statusClasses[entry.status]}`}>{t(statusLabels[entry.status])}</span></div>
            </article>
          ))}
        </section>
      )}
    </div>
  );
}
