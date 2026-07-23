export type ProcessingHistoryCategory = "Image" | "PDF" | "Video";
export type ProcessingHistoryStatus = "Completed" | "Failed" | "Cancelled";
export type ImageProcessingHistoryTool = "Compress Images" | "Convert Images" | "Resize Images";
export type PdfProcessingHistoryTool =
  | "Merge PDFs"
  | "Split PDF"
  | "Organize Pages"
  | "Images to PDF"
  | "PDF to Images";

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

export interface ImageProcessingHistoryInput {
  id?: string;
  datetime?: string;
  tool: ImageProcessingHistoryTool;
  sourceFileName: string;
  resultPath?: string;
  success: boolean;
  cancelled?: boolean;
  originalSize?: number;
  finalSize?: number;
  durationMs?: number;
  errorMessage?: string;
}

export interface PdfProcessingHistoryInput {
  id?: string;
  datetime?: string;
  tool: PdfProcessingHistoryTool;
  sourceFileName: string;
  resultPath?: string;
  success: boolean;
  cancelled?: boolean;
  originalSize?: number;
  finalSize?: number;
  durationMs?: number;
  errorMessage?: string;
}

export interface ProcessingHistoryChangedDetail {
  entries: ProcessingHistoryEntry[];
}

export const PROCESSING_HISTORY_CHANGED_EVENT = "filenova-processing-history-changed";

const PROCESSING_HISTORY_STORAGE_KEY = "filenova-processing-history";
const MAX_HISTORY_ENTRIES = 500;
let mutationQueue: Promise<void> = Promise.resolve();

function isHistoryEntry(entry: ProcessingHistoryEntry): boolean {
  const hasRequiredFields = (
    typeof entry?.id === "string"
    && entry.id.length > 0
    && typeof entry?.datetime === "string"
    && ["Image", "PDF", "Video"].includes(entry?.category)
    && typeof entry?.tool === "string"
    && entry.tool.length > 0
    && typeof entry?.sourceFileName === "string"
    && entry.sourceFileName.length > 0
    && typeof entry?.createdFileName === "string"
    && typeof entry?.createdFilePath === "string"
    && ["Completed", "Failed", "Cancelled"].includes(entry?.status)
  );
  return hasRequiredFields && (
    entry.status !== "Completed"
    || (entry.createdFileName.length > 0 && entry.createdFilePath.length > 0)
  );
}

function deduplicateEntries(entries: ProcessingHistoryEntry[]): ProcessingHistoryEntry[] {
  const seen = new Set<string>();
  return entries.filter((entry) => {
    if (!isHistoryEntry(entry) || seen.has(entry.id)) return false;
    seen.add(entry.id);
    return true;
  });
}

function parseHistory(raw: string | null): ProcessingHistoryEntry[] {
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw) as ProcessingHistoryEntry[];
    if (!Array.isArray(parsed)) return [];
    return deduplicateEntries(parsed).slice(-MAX_HISTORY_ENTRIES);
  } catch {
    return [];
  }
}

function readHistoryState(): { entries: ProcessingHistoryEntry[]; readable: boolean } {
  try {
    return {
      entries: parseHistory(localStorage.getItem(PROCESSING_HISTORY_STORAGE_KEY)),
      readable: true,
    };
  } catch {
    return { entries: [], readable: false };
  }
}

function readHistorySafely(): ProcessingHistoryEntry[] {
  return readHistoryState().entries;
}

function emitHistoryChanged(entries: ProcessingHistoryEntry[]) {
  try {
    window.dispatchEvent(new CustomEvent<ProcessingHistoryChangedDetail>(
      PROCESSING_HISTORY_CHANGED_EVENT,
      { detail: { entries: [...entries] } },
    ));
  } catch {
    // History notifications are best effort and must never affect processing.
  }
}

function serializeHistory(entries: ProcessingHistoryEntry[]): boolean {
  try {
    localStorage.setItem(PROCESSING_HISTORY_STORAGE_KEY, JSON.stringify(entries));
    emitHistoryChanged(entries);
    return true;
  } catch {
    return false;
  }
}

function enqueueMutation(operation: () => ProcessingHistoryEntry[]): Promise<ProcessingHistoryEntry[]> {
  const result = mutationQueue.then(() => {
    try {
      return operation();
    } catch {
      return readHistorySafely();
    }
  });
  mutationQueue = result.then(
    () => undefined,
    () => undefined,
  );
  return result.catch(() => readHistorySafely());
}

function newHistoryId(): string {
  try {
    if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
  } catch {
    // Use the local fallback when randomUUID is unavailable.
  }
  return `history-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function fileNameFromPath(path: string): string {
  return path.split(/[\\/]/).pop() ?? "";
}

function safeDuration(value?: number): number {
  return Number.isFinite(value) ? Math.max(0, value ?? 0) : 0;
}

export function createImageProcessingHistoryEntry(input: ImageProcessingHistoryInput): ProcessingHistoryEntry {
  const resultPath = input.resultPath?.trim() ?? "";
  const completed = !input.cancelled && input.success && resultPath.length > 0;
  const status: ProcessingHistoryStatus = input.cancelled
    ? "Cancelled"
    : completed
      ? "Completed"
      : "Failed";
  const createdFilePath = completed ? resultPath : "";

  return {
    id: input.id ?? newHistoryId(),
    datetime: input.datetime ?? new Date().toISOString(),
    category: "Image",
    tool: input.tool,
    sourceFileName: input.sourceFileName,
    createdFileName: fileNameFromPath(createdFilePath),
    createdFilePath,
    status,
    originalSize: input.originalSize,
    finalSize: input.finalSize,
    durationMs: input.durationMs,
    errorMessage: status === "Failed" && input.success && !resultPath
      ? input.errorMessage ?? "Processing reported success without a result path."
      : input.errorMessage,
  };
}

export function createPdfProcessingHistoryEntry(input: PdfProcessingHistoryInput): ProcessingHistoryEntry {
  const resultPath = input.resultPath?.trim() ?? "";
  const completed = !input.cancelled && input.success && resultPath.length > 0;
  const status: ProcessingHistoryStatus = input.cancelled
    ? "Cancelled"
    : completed
      ? "Completed"
      : "Failed";
  const createdFilePath = completed ? resultPath : "";

  return {
    id: input.id ?? newHistoryId(),
    datetime: input.datetime ?? new Date().toISOString(),
    category: "PDF",
    tool: input.tool,
    sourceFileName: input.sourceFileName,
    createdFileName: fileNameFromPath(createdFilePath),
    createdFilePath,
    status,
    originalSize: input.originalSize,
    finalSize: completed ? input.finalSize : undefined,
    durationMs: safeDuration(input.durationMs),
    errorMessage: status === "Failed" && input.success && !resultPath
      ? input.errorMessage ?? "Processing reported success without a result path."
      : input.errorMessage,
  };
}

export async function loadProcessingHistory(): Promise<ProcessingHistoryEntry[]> {
  return readHistorySafely();
}

export function saveProcessingHistory(entries: ProcessingHistoryEntry[]): Promise<ProcessingHistoryEntry[]> {
  return enqueueMutation(() => {
    const current = readHistoryState();
    if (!current.readable) return [];
    const normalized = deduplicateEntries(entries).slice(-MAX_HISTORY_ENTRIES);
    return serializeHistory(normalized) ? normalized : current.entries;
  });
}

export function clearProcessingHistory(): Promise<ProcessingHistoryEntry[]> {
  return saveProcessingHistory([]);
}

export function appendProcessingHistoryEntry(
  entry: ProcessingHistoryEntry,
  enabled = true,
): Promise<ProcessingHistoryEntry[]> {
  return appendProcessingHistoryEntries([entry], enabled);
}

export function appendProcessingHistoryEntries(
  entries: ProcessingHistoryEntry[],
  enabled = true,
): Promise<ProcessingHistoryEntry[]> {
  if (!enabled || entries.length === 0) {
    return loadProcessingHistory();
  }

  return enqueueMutation(() => {
    const current = readHistoryState();
    if (!current.readable) return [];
    const existing = current.entries;
    const seen = new Set(existing.map((item) => item.id));
    const uniqueEntries = entries.filter((entry) => {
      if (!isHistoryEntry(entry) || seen.has(entry.id)) return false;
      seen.add(entry.id);
      return true;
    });
    if (uniqueEntries.length === 0) return existing;

    const updated = [...existing, ...uniqueEntries].slice(-MAX_HISTORY_ENTRIES);
    return serializeHistory(updated) ? updated : existing;
  });
}
