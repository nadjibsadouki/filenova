export type ProcessingHistoryCategory = "Image" | "PDF" | "Video";
export type ProcessingHistoryStatus = "Completed" | "Failed" | "Cancelled";

export interface ProcessingHistoryEntry {
  id: string;
  datetime: string;
  category: ProcessingHistoryCategory;
  tool: string;
  sourceFileName: string;
  createdFileName: string;
  createdFilePath: string;
  status: ProcessingHistoryStatus;
  originalSize?: number;
  finalSize?: number;
  durationMs?: number;
  errorMessage?: string;
}

const PROCESSING_HISTORY_STORAGE_KEY = "filenova-processing-history";
const MAX_HISTORY_ENTRIES = 500;

function parseHistory(raw: string | null): ProcessingHistoryEntry[] {
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw) as ProcessingHistoryEntry[];
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((entry): entry is ProcessingHistoryEntry => (
      typeof entry?.id === "string"
      && typeof entry?.datetime === "string"
      && ["Image", "PDF", "Video"].includes(entry?.category)
      && typeof entry?.tool === "string"
      && typeof entry?.sourceFileName === "string"
      && typeof entry?.createdFileName === "string"
      && typeof entry?.createdFilePath === "string"
      && ["Completed", "Failed", "Cancelled"].includes(entry?.status)
    ));
  } catch {
    return [];
  }
}

function serializeHistory(entries: ProcessingHistoryEntry[]) {
  try {
    localStorage.setItem(PROCESSING_HISTORY_STORAGE_KEY, JSON.stringify(entries));
  } catch {
    // Best effort only.
  }
}

export async function loadProcessingHistory(): Promise<ProcessingHistoryEntry[]> {
  return parseHistory(localStorage.getItem(PROCESSING_HISTORY_STORAGE_KEY));
}

export async function saveProcessingHistory(entries: ProcessingHistoryEntry[]): Promise<ProcessingHistoryEntry[]> {
  const trimmed = entries.slice(-MAX_HISTORY_ENTRIES);
  serializeHistory(trimmed);
  return trimmed;
}

export async function appendProcessingHistoryEntry(entry: ProcessingHistoryEntry, enabled = true): Promise<ProcessingHistoryEntry[]> {
  return appendProcessingHistoryEntries([entry], enabled);
}

export async function appendProcessingHistoryEntries(entries: ProcessingHistoryEntry[], enabled = true): Promise<ProcessingHistoryEntry[]> {
  if (!enabled || entries.length === 0) {
    return loadProcessingHistory();
  }

  const existing = await loadProcessingHistory();
  const existingIds = new Set(existing.map((item) => item.id));
  const uniqueEntries = entries.filter((entry) => !existingIds.has(entry.id));
  if (!uniqueEntries.length) {
    return existing;
  }

  const updated = [...existing, ...uniqueEntries].slice(-MAX_HISTORY_ENTRIES);
  serializeHistory(updated);
  return updated;
}
