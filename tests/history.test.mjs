import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test, { beforeEach } from "node:test";
import ts from "typescript";

class MemoryStorage {
  values = new Map();
  failRead = false;
  failWrite = false;

  getItem(key) {
    if (this.failRead) throw new Error("read unavailable");
    return this.values.get(key) ?? null;
  }

  setItem(key, value) {
    if (this.failWrite) throw new Error("write unavailable");
    this.values.set(key, value);
  }

  clear() {
    this.values.clear();
    this.failRead = false;
    this.failWrite = false;
  }
}

class TestCustomEvent extends Event {
  constructor(type, init = {}) {
    super(type);
    this.detail = init.detail;
  }
}

const storage = new MemoryStorage();
const eventTarget = new EventTarget();
globalThis.localStorage = storage;
globalThis.CustomEvent = TestCustomEvent;
globalThis.window = {
  addEventListener: eventTarget.addEventListener.bind(eventTarget),
  removeEventListener: eventTarget.removeEventListener.bind(eventTarget),
  dispatchEvent: eventTarget.dispatchEvent.bind(eventTarget),
};

const historySource = await readFile(new URL("../src/history.ts", import.meta.url), "utf8");
const compiledHistory = ts.transpileModule(historySource, {
  compilerOptions: {
    module: ts.ModuleKind.ES2022,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const history = await import(`data:text/javascript;base64,${Buffer.from(compiledHistory).toString("base64")}`);

const {
  PROCESSING_HISTORY_CHANGED_EVENT,
  appendProcessingHistoryEntry,
  appendProcessingHistoryEntries,
  clearProcessingHistory,
  createImageProcessingHistoryEntry,
  createPdfProcessingHistoryEntry,
  loadProcessingHistory,
  saveProcessingHistory,
} = history;

function completedEntry(id, overrides = {}) {
  return createImageProcessingHistoryEntry({
    id,
    datetime: "2026-07-23T10:20:30.000Z",
    tool: "Compress Images",
    sourceFileName: "source image.png",
    resultPath: "C:\\Résultats FileNova\\source image-compressed.png",
    success: true,
    originalSize: 2_000,
    finalSize: 1_000,
    durationMs: 42,
    ...overrides,
  });
}

beforeEach(async () => {
  storage.clear();
  await clearProcessingHistory();
});

test("adds one complete history entry with every Image field", async () => {
  const entry = completedEntry("one");
  const entries = await appendProcessingHistoryEntry(entry);

  assert.deepEqual(entries, [entry]);
  assert.equal(entries[0].category, "Image");
  assert.equal(entries[0].status, "Completed");
  assert.equal(entries[0].createdFileName, "source image-compressed.png");
  assert.equal(entries[0].createdFilePath, "C:\\Résultats FileNova\\source image-compressed.png");
  assert.equal(entries[0].originalSize, 2_000);
  assert.equal(entries[0].finalSize, 1_000);
  assert.equal(entries[0].durationMs, 42);
});

test("adds one entry per file in a batch", async () => {
  const entries = await appendProcessingHistoryEntries([
    completedEntry("batch-1", { sourceFileName: "un.png" }),
    completedEntry("batch-2", { sourceFileName: "deux.png" }),
    completedEntry("batch-3", { sourceFileName: "trois.png" }),
  ]);

  assert.equal(entries.length, 3);
  assert.deepEqual(entries.map((entry) => entry.sourceFileName), ["un.png", "deux.png", "trois.png"]);
});

test("rejects an identifier already present in history", async () => {
  await appendProcessingHistoryEntry(completedEntry("duplicate"));
  const entries = await appendProcessingHistoryEntry(completedEntry("duplicate", { sourceFileName: "other.png" }));

  assert.equal(entries.length, 1);
  assert.equal(entries[0].sourceFileName, "source image.png");
});

test("rejects duplicate identifiers inside the same batch", async () => {
  const entries = await appendProcessingHistoryEntries([
    completedEntry("same-batch"),
    completedEntry("same-batch", { sourceFileName: "duplicate.png" }),
  ]);

  assert.equal(entries.length, 1);
});

test("keeps only the 500 most recent entries", async () => {
  const input = Array.from({ length: 501 }, (_, index) => completedEntry(`limit-${index}`));
  const entries = await saveProcessingHistory(input);

  assert.equal(entries.length, 500);
  assert.equal(entries[0].id, "limit-1");
  assert.equal(entries.at(-1).id, "limit-500");
});

test("does not write when processing history is disabled", async () => {
  await appendProcessingHistoryEntry(completedEntry("kept"));
  const entries = await appendProcessingHistoryEntry(completedEntry("disabled"), false);

  assert.deepEqual(entries.map((entry) => entry.id), ["kept"]);
});

test("returns an empty history when localStorage cannot be read", async () => {
  storage.failRead = true;
  assert.deepEqual(await loadProcessingHistory(), []);
});

test("does not overwrite storage when the previous history cannot be read", async () => {
  await appendProcessingHistoryEntry(completedEntry("protected"));
  storage.failRead = true;
  assert.deepEqual(await appendProcessingHistoryEntry(completedEntry("unsafe-write")), []);
  storage.failRead = false;

  assert.deepEqual((await loadProcessingHistory()).map((entry) => entry.id), ["protected"]);
});

test("keeps the previous safe state when localStorage cannot be written", async () => {
  await appendProcessingHistoryEntry(completedEntry("existing"));
  storage.failWrite = true;
  const entries = await appendProcessingHistoryEntry(completedEntry("not-written"));
  storage.failWrite = false;

  assert.deepEqual(entries.map((entry) => entry.id), ["existing"]);
  assert.deepEqual((await loadProcessingHistory()).map((entry) => entry.id), ["existing"]);
});

test("serializes simultaneous additions without losing an entry", async () => {
  await Promise.all([
    appendProcessingHistoryEntry(completedEntry("concurrent-a")),
    appendProcessingHistoryEntry(completedEntry("concurrent-b")),
  ]);

  assert.deepEqual(
    (await loadProcessingHistory()).map((entry) => entry.id),
    ["concurrent-a", "concurrent-b"],
  );
});

test("emits a local update event after additions and cleanup", async () => {
  const snapshots = [];
  const listener = (event) => snapshots.push(event.detail.entries.map((entry) => entry.id));
  window.addEventListener(PROCESSING_HISTORY_CHANGED_EVENT, listener);

  await appendProcessingHistoryEntry(completedEntry("event-entry"));
  await clearProcessingHistory();
  window.removeEventListener(PROCESSING_HISTORY_CHANGED_EVENT, listener);

  assert.deepEqual(snapshots, [["event-entry"], []]);
});

test("uses Completed only for a successful non-empty result path", () => {
  assert.equal(completedEntry("completed").status, "Completed");
  const missingResult = completedEntry("missing-result", { resultPath: "", success: true });
  assert.equal(missingResult.status, "Failed");
  assert.equal(missingResult.createdFileName, "");
  assert.equal(missingResult.createdFilePath, "");
});

test("records processing errors as Failed without a result", () => {
  const entry = completedEntry("failed", {
    success: false,
    resultPath: undefined,
    finalSize: undefined,
    errorMessage: "Impossible de décoder l’image.",
  });

  assert.equal(entry.status, "Failed");
  assert.equal(entry.errorMessage, "Impossible de décoder l’image.");
  assert.equal(entry.createdFilePath, "");
});

test("records cancellation as Cancelled without a result", () => {
  const entry = completedEntry("cancelled", {
    success: false,
    cancelled: true,
    resultPath: undefined,
    finalSize: 0,
    durationMs: 0,
  });

  assert.equal(entry.status, "Cancelled");
  assert.equal(entry.createdFileName, "");
  assert.equal(entry.createdFilePath, "");
});

test("records exact Image tool names and Unicode paths for all three tools", async () => {
  const entries = await appendProcessingHistoryEntries([
    completedEntry("compress", {
      tool: "Compress Images",
      sourceFileName: "vacances été 你好.png",
      resultPath: "C:\\Mes images\\été 你好-compressed.png",
    }),
    completedEntry("convert", {
      tool: "Convert Images",
      sourceFileName: "élève numéro 2.png",
      resultPath: "C:\\Résultats accentués\\élève numéro 2-converted.webp",
    }),
    completedEntry("resize", {
      tool: "Resize Images",
      sourceFileName: "صورة نهائية.png",
      resultPath: "C:\\Dossier avec espaces\\صورة نهائية-resized.png",
    }),
  ]);

  assert.deepEqual(entries.map((entry) => entry.tool), [
    "Compress Images",
    "Convert Images",
    "Resize Images",
  ]);
  assert.deepEqual(entries.map((entry) => entry.category), ["Image", "Image", "Image"]);
  assert.match(entries[0].createdFilePath, /été 你好/);
  assert.match(entries[1].createdFileName, /élève numéro 2/);
  assert.match(entries[2].createdFilePath, /صورة نهائية/);
});

test("history integration never changes an original Unicode source file", async () => {
  const directory = await mkdtemp(join(tmpdir(), "filenova historique "));
  const sourcePath = join(directory, "vacances été 你好.png");
  const original = Buffer.from([0, 1, 2, 3, 254, 255]);
  try {
    await writeFile(sourcePath, original);
    await appendProcessingHistoryEntries([
      completedEntry("original-compress", { tool: "Compress Images", sourceFileName: sourcePath }),
      completedEntry("original-convert", { tool: "Convert Images", sourceFileName: sourcePath }),
      completedEntry("original-resize", { tool: "Resize Images", sourceFileName: sourcePath }),
    ]);

    assert.deepEqual(await readFile(sourcePath), original);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("PDF constructor supports all five canonical tools and safe durations", () => {
  const tools = ["Merge PDFs", "Split PDF", "Organize Pages", "Images to PDF", "PDF to Images"];
  const entries = tools.map((tool, index) => createPdfProcessingHistoryEntry({
    id: `pdf-tool-${index}`,
    tool,
    sourceFileName: "document source.pdf",
    resultPath: `C:\\Résultats PDF\\sortie ${index}.pdf`,
    success: true,
    originalSize: 4_000,
    finalSize: 2_000,
    durationMs: index === 0 ? -10 : index,
  }));

  assert.deepEqual(entries.map((entry) => entry.tool), tools);
  assert.deepEqual(entries.map((entry) => entry.category), tools.map(() => "PDF"));
  assert.equal(entries[0].durationMs, 0);
  assert.equal(entries[4].durationMs, 4);
});

test("PDF Completed requires success and a non-empty Windows result path", () => {
  const completed = createPdfProcessingHistoryEntry({
    id: "pdf-completed",
    tool: "Organize Pages",
    sourceFileName: "dôcument source.pdf",
    resultPath: "C:\\Dossier avec espaces\\résultat organisé.pdf",
    success: true,
    durationMs: 12,
  });
  const missing = createPdfProcessingHistoryEntry({
    id: "pdf-missing",
    tool: "Organize Pages",
    sourceFileName: "dôcument source.pdf",
    resultPath: " ",
    success: true,
    durationMs: 12,
  });

  assert.equal(completed.status, "Completed");
  assert.equal(completed.createdFileName, "résultat organisé.pdf");
  assert.equal(missing.status, "Failed");
  assert.equal(missing.createdFileName, "");
  assert.equal(missing.createdFilePath, "");
});

test("PDF Failed and Cancelled entries never retain result data", () => {
  const failed = createPdfProcessingHistoryEntry({
    id: "pdf-failed",
    tool: "Merge PDFs",
    sourceFileName: "un.pdf • deux.pdf",
    resultPath: "C:\\should-not-be-kept.pdf",
    success: false,
    finalSize: 100,
    durationMs: 7,
    errorMessage: "Fusion impossible.",
  });
  const cancelled = createPdfProcessingHistoryEntry({
    id: "pdf-cancelled",
    tool: "Split PDF",
    sourceFileName: "source.pdf",
    resultPath: "C:\\should-not-be-kept.pdf",
    success: false,
    cancelled: true,
    durationMs: 0,
  });

  assert.equal(failed.status, "Failed");
  assert.equal(failed.createdFilePath, "");
  assert.equal(failed.finalSize, undefined);
  assert.equal(cancelled.status, "Cancelled");
  assert.equal(cancelled.createdFilePath, "");
});

test("Merge PDFs history records ordered sources, summed size and one output", async () => {
  const entry = createPdfProcessingHistoryEntry({
    id: "merge-history",
    tool: "Merge PDFs",
    sourceFileName: ["premier.pdf", "deuxième.pdf", "وثيقة.pdf"].join(" • "),
    resultPath: "C:\\PDF fusionnés\\résultat final.pdf",
    success: true,
    originalSize: 100 + 200 + 300,
    finalSize: 450,
    durationMs: 25,
  });
  const entries = await appendProcessingHistoryEntry(entry);

  assert.equal(entries.length, 1);
  assert.equal(entries[0].originalSize, 600);
  assert.equal(entries[0].sourceFileName, "premier.pdf • deuxième.pdf • وثيقة.pdf");
});

test("Split PDF history records one unique entry per real output and one failure without output", async () => {
  const outputs = ["partie 1.pdf", "partie 2.pdf", "partie 3.pdf"].map((name, index) => (
    createPdfProcessingHistoryEntry({
      id: `split-${index}`,
      tool: "Split PDF",
      sourceFileName: "document spécial.pdf",
      resultPath: `C:\\Découpage PDF\\${name}`,
      success: true,
      originalSize: 900,
      finalSize: 300,
      durationMs: 40,
    })
  ));
  await appendProcessingHistoryEntries([...outputs, outputs[0]]);
  await appendProcessingHistoryEntry(createPdfProcessingHistoryEntry({
    id: "split-failure",
    tool: "Split PDF",
    sourceFileName: "cassé.pdf",
    success: false,
    originalSize: 50,
    durationMs: 3,
    errorMessage: "Découpage impossible.",
  }));
  const entries = await loadProcessingHistory();

  assert.equal(entries.filter((entry) => entry.status === "Completed").length, 3);
  assert.equal(entries.filter((entry) => entry.status === "Failed").length, 1);
  assert.equal(entries.at(-1).createdFilePath, "");
});

test("Organize Pages and Images to PDF produce one entry and preserve source totals", async () => {
  const organized = createPdfProcessingHistoryEntry({
    id: "organize-history",
    tool: "Organize Pages",
    sourceFileName: "original intact.pdf",
    resultPath: "C:\\Sorties\\original intact-organized.pdf",
    success: true,
    originalSize: 700,
    finalSize: 650,
    durationMs: 15,
  });
  const imagesPdf = createPdfProcessingHistoryEntry({
    id: "images-pdf-history",
    tool: "Images to PDF",
    sourceFileName: ["été 你好.png", "صورة نهائية.jpg"].join(" • "),
    resultPath: "C:\\Sorties\\images réunies.pdf",
    success: true,
    originalSize: 120 + 380,
    finalSize: 420,
    durationMs: 22,
  });
  const entries = await appendProcessingHistoryEntries([organized, imagesPdf]);

  assert.equal(entries.length, 2);
  assert.equal(entries[0].originalSize, 700);
  assert.equal(entries[1].originalSize, 500);
});

test("PDF to Images records PNG, JPG, WebP outputs and one failed page in category PDF", async () => {
  const formats = ["png", "jpg", "webp"];
  const entries = formats.map((format, index) => createPdfProcessingHistoryEntry({
    id: `pdf-image-${format}`,
    tool: "PDF to Images",
    sourceFileName: `catalogue.pdf (page ${index + 1})`,
    resultPath: `C:\\Images PDF\\catalogue-page-${index + 1}.${format}`,
    success: true,
    originalSize: 1_500,
    finalSize: 200 + index,
    durationMs: 80,
  }));
  entries.push(createPdfProcessingHistoryEntry({
    id: "pdf-image-failed-page",
    tool: "PDF to Images",
    sourceFileName: "catalogue.pdf (page 4)",
    success: false,
    originalSize: 1_500,
    durationMs: 80,
    errorMessage: "Page 4: rendu impossible.",
  }));
  const stored = await appendProcessingHistoryEntries(entries);

  assert.equal(stored.length, 4);
  assert.ok(stored.every((entry) => entry.category === "PDF"));
  assert.deepEqual(stored.slice(0, 3).map((entry) => entry.createdFileName.split(".").at(-1)), formats);
  assert.equal(stored[3].status, "Failed");
  assert.match(stored[3].errorMessage, /Page 4/);
});

test("disabled or failing storage never blocks a PDF history call", async () => {
  const entry = createPdfProcessingHistoryEntry({
    id: "pdf-safe-history",
    tool: "Merge PDFs",
    sourceFileName: "a.pdf • b.pdf",
    resultPath: "C:\\Sorties\\merged.pdf",
    success: true,
    durationMs: 1,
  });
  assert.deepEqual(await appendProcessingHistoryEntry(entry, false), []);

  storage.failWrite = true;
  await assert.doesNotReject(appendProcessingHistoryEntry(entry, true));
  storage.failWrite = false;
  assert.deepEqual(await loadProcessingHistory(), []);
});
