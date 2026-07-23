import type {
  ProcessingHistoryCategory,
  ProcessingHistoryEntry,
  ProcessingHistoryStatus,
} from "./history";

export interface ProcessingResultAvailability {
  fileExists: boolean;
  parentExists: boolean;
  parentPath?: string | null;
}

export type ProcessingHistoryCategoryFilter = "all" | ProcessingHistoryCategory;
export type ProcessingHistoryStatusFilter = "all" | ProcessingHistoryStatus;

export interface ProcessingHistoryFilters {
  category: ProcessingHistoryCategoryFilter;
  status: ProcessingHistoryStatusFilter;
  search: string;
}

export const DEFAULT_PROCESSING_HISTORY_FILTERS: ProcessingHistoryFilters = {
  category: "all",
  status: "all",
  search: "",
};

export function filterProcessingHistory(
  entries: ProcessingHistoryEntry[],
  filters: ProcessingHistoryFilters,
): ProcessingHistoryEntry[] {
  const query = filters.search.trim().toLocaleLowerCase();
  return entries.filter((entry) => {
    const categoryMatches = filters.category === "all" || entry.category === filters.category;
    const statusMatches = filters.status === "all" || entry.status === filters.status;
    const searchMatches = !query || [
      entry.tool,
      entry.sourceFileName,
      entry.createdFileName,
      entry.createdFilePath,
      entry.details ?? "",
      entry.errorMessage ?? "",
    ].some((value) => value.toLocaleLowerCase().includes(query));
    return categoryMatches && statusMatches && searchMatches;
  });
}

export function formatHistoryBytes(value?: number): string {
  if (!Number.isFinite(value) || value === undefined || value < 0) return "—";
  const units = ["B", "KB", "MB", "GB"] as const;
  let amount = value;
  let unit = 0;
  while (amount >= 1024 && unit < units.length - 1) {
    amount /= 1024;
    unit += 1;
  }
  const precision = unit === 0 || amount >= 10 ? 0 : 1;
  return `${amount.toFixed(precision)} ${units[unit]}`;
}

export function formatHistoryDuration(value?: number): string {
  if (!Number.isFinite(value) || value === undefined || value < 0) return "—";
  if (value < 1_000) return `${Math.round(value)} ms`;
  const totalSeconds = value / 1_000;
  if (totalSeconds < 60) return `${totalSeconds.toFixed(totalSeconds >= 10 ? 1 : 2)} s`;
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = Math.round(totalSeconds % 60);
  return `${minutes} min ${seconds} s`;
}

export function formatHistoryDate(value: string, locale: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  try {
    return new Intl.DateTimeFormat(locale, {
      dateStyle: "medium",
      timeStyle: "medium",
    }).format(date);
  } catch {
    return date.toLocaleString();
  }
}

export function canOpenProcessingResult(
  entry: ProcessingHistoryEntry,
  availability?: ProcessingResultAvailability,
): boolean {
  return entry.status === "Completed"
    && entry.createdFilePath.trim().length > 0
    && availability?.fileExists === true;
}
