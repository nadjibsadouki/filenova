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
const historyViewSource = await readFile(new URL("../src/historyView.ts", import.meta.url), "utf8");
const compiledHistoryView = ts.transpileModule(historyViewSource, {
  compilerOptions: {
    module: ts.ModuleKind.ES2022,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const historyView = await import(`data:text/javascript;base64,${Buffer.from(compiledHistoryView).toString("base64")}`);

const {
  PROCESSING_HISTORY_CHANGED_EVENT,
  appendProcessingHistoryEntry,
  appendProcessingHistoryEntries,
  clearProcessingHistory,
  clearProcessingHistoryWithResult,
  createImageProcessingHistoryEntry,
  createPdfProcessingHistoryEntry,
  createVideoProcessingHistoryEntry,
  loadProcessingHistory,
  loadProcessingHistoryWithResult,
  removeProcessingHistoryEntry,
  saveProcessingHistory,
} = history;
const {
  canOpenProcessingResult,
  DEFAULT_PROCESSING_HISTORY_FILTERS,
  filterProcessingHistory,
  formatHistoryBytes,
  formatHistoryDate,
  formatHistoryDuration,
} = historyView;

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

test("Video constructor uses canonical tools, unique identifiers, ISO dates and safe durations", () => {
  const compressed = createVideoProcessingHistoryEntry({
    tool: "Compress Video",
    sourceFileName: "source.mp4",
    resultPath: "C:\\Videos\\source-compressed.mp4",
    success: true,
    durationMs: -12,
  });
  const converted = createVideoProcessingHistoryEntry({
    tool: "Convert Video",
    sourceFileName: "source.mp4",
    resultPath: "C:\\Videos\\source-converted.webm",
    success: true,
    durationMs: Number.NaN,
  });

  assert.equal(compressed.category, "Video");
  assert.equal(converted.category, "Video");
  assert.deepEqual([compressed.tool, converted.tool], ["Compress Video", "Convert Video"]);
  assert.notEqual(compressed.id, converted.id);
  assert.equal(new Date(compressed.datetime).toISOString(), compressed.datetime);
  assert.equal(compressed.durationMs, 0);
  assert.equal(converted.durationMs, 0);
});

test("Compress Video records Completed with sizes and a Unicode final path", () => {
  const entry = createVideoProcessingHistoryEntry({
    id: "video-compress-completed",
    tool: "Compress Video",
    sourceFileName: "vacances \u00e9t\u00e9 \u4f60\u597d.mp4",
    resultPath: "C:\\Vid\u00e9os avec espaces\\vacances \u00e9t\u00e9 \u4f60\u597d-compressed.mp4",
    success: true,
    originalSize: 9_000,
    finalSize: 4_000,
    durationMs: 125,
  });

  assert.equal(entry.status, "Completed");
  assert.equal(entry.createdFileName, "vacances \u00e9t\u00e9 \u4f60\u597d-compressed.mp4");
  assert.equal(entry.originalSize, 9_000);
  assert.equal(entry.finalSize, 4_000);
  assert.equal(entry.durationMs, 125);
});

test("Compress Video records Failed and Cancelled without temporary or result paths", () => {
  const failed = createVideoProcessingHistoryEntry({
    id: "video-compress-failed",
    tool: "Compress Video",
    sourceFileName: "cass\u00e9.mp4",
    resultPath: "C:\\Sorties\\partial.tmp.mp4",
    success: false,
    originalSize: 200,
    finalSize: 30,
    durationMs: 8,
    errorMessage: "FFmpeg failed.",
  });
  const cancelled = createVideoProcessingHistoryEntry({
    id: "video-compress-cancelled",
    tool: "Compress Video",
    sourceFileName: "annul\u00e9.mp4",
    resultPath: "C:\\Sorties\\deleted.tmp.mp4",
    success: false,
    cancelled: true,
    originalSize: 300,
    finalSize: 40,
    durationMs: 9,
  });

  assert.deepEqual([failed.status, cancelled.status], ["Failed", "Cancelled"]);
  assert.ok([failed, cancelled].every((entry) => entry.createdFilePath === ""));
  assert.ok([failed, cancelled].every((entry) => entry.createdFileName === ""));
  assert.ok([failed, cancelled].every((entry) => entry.finalSize === undefined));
});

test("Convert Video records Completed, Failed and Cancelled with the selected output format", () => {
  const completed = createVideoProcessingHistoryEntry({
    id: "video-convert-completed",
    tool: "Convert Video",
    sourceFileName: "clip source.mov",
    resultPath: "C:\\R\u00e9sultats vid\u00e9o\\clip source-converted.webm",
    success: true,
    originalSize: 6_000,
    finalSize: 3_500,
    durationMs: 250,
  });
  const failed = createVideoProcessingHistoryEntry({
    id: "video-convert-failed",
    tool: "Convert Video",
    sourceFileName: "invalide.avi",
    success: false,
    originalSize: 100,
    durationMs: 5,
    errorMessage: "Invalid video.",
  });
  const cancelled = createVideoProcessingHistoryEntry({
    id: "video-convert-cancelled",
    tool: "Convert Video",
    sourceFileName: "longue vid\u00e9o.mp4",
    success: false,
    cancelled: true,
    originalSize: 8_000,
    durationMs: 20,
  });

  assert.equal(completed.status, "Completed");
  assert.match(completed.createdFileName, /\.webm$/);
  assert.equal(failed.status, "Failed");
  assert.equal(failed.errorMessage, "Invalid video.");
  assert.equal(cancelled.status, "Cancelled");
  assert.ok([failed, cancelled].every((entry) => entry.createdFilePath === ""));
});

test("Video batches keep one entry per file and reject duplicate identifiers", async () => {
  const entries = [
    createVideoProcessingHistoryEntry({
      id: "video-batch-1",
      tool: "Compress Video",
      sourceFileName: "un.mp4",
      resultPath: "C:\\Sorties\\un-compressed.mp4",
      success: true,
    }),
    createVideoProcessingHistoryEntry({
      id: "video-batch-2",
      tool: "Convert Video",
      sourceFileName: "deux.mov",
      resultPath: "C:\\Sorties\\deux-converted.webm",
      success: true,
    }),
  ];
  await appendProcessingHistoryEntries([...entries, entries[0]]);
  await appendProcessingHistoryEntry(entries[1]);

  const stored = await loadProcessingHistory();
  assert.equal(stored.length, 2);
  assert.deepEqual(stored.map((entry) => entry.sourceFileName), ["un.mp4", "deux.mov"]);
});

test("disabled or failing history storage never blocks Video processing results", async () => {
  const entry = createVideoProcessingHistoryEntry({
    id: "video-safe-history",
    tool: "Convert Video",
    sourceFileName: "source.mp4",
    resultPath: "C:\\Sorties\\source-converted.webm",
    success: true,
  });

  assert.deepEqual(await appendProcessingHistoryEntry(entry, false), []);
  storage.failWrite = true;
  await assert.doesNotReject(appendProcessingHistoryEntries([entry], true));
  storage.failWrite = false;
  assert.deepEqual(await loadProcessingHistory(), []);
});

test("Video history leaves an original file with spaces, accents and Unicode intact", async () => {
  const directory = await mkdtemp(join(tmpdir(), "filenova vid\u00e9o history "));
  const sourcePath = join(directory, "source \u00e9t\u00e9 \u4f60\u597d.mp4");
  const original = Buffer.from([9, 8, 7, 6, 5, 4]);
  try {
    await writeFile(sourcePath, original);
    await appendProcessingHistoryEntries([
      createVideoProcessingHistoryEntry({
        tool: "Compress Video",
        sourceFileName: sourcePath,
        resultPath: join(directory, "compressed.mp4"),
        success: true,
      }),
      createVideoProcessingHistoryEntry({
        tool: "Convert Video",
        sourceFileName: sourcePath,
        resultPath: join(directory, "converted.webm"),
        success: true,
      }),
    ]);

    assert.deepEqual(await readFile(sourcePath), original);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("Compress Video and Convert Video pages batch history without awaiting it", async () => {
  const [compressSource, convertSource] = await Promise.all([
    readFile(new URL("../src/pages/CompressVideoPage.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/pages/ConvertVideoPage.tsx", import.meta.url), "utf8"),
  ]);

  for (const [source, tool] of [
    [compressSource, "Compress Video"],
    [convertSource, "Convert Video"],
  ]) {
    assert.match(source, /useSettings/);
    assert.match(source, /void appendProcessingHistoryEntries/);
    assert.match(source, /settings\.enableProcessingHistory/);
    assert.match(source, new RegExp(`tool: "${tool}"`));
    assert.match(source, /cancelled/);
  }
  assert.match(compressSource, /if \(!result\.pendingConfirmation\)/);
  assert.match(compressSource, /resultPath: kept\.outputPath/);
});

test("Extract Audio Completed records the final format, sizes, duration and Unicode path", () => {
  const entry = createVideoProcessingHistoryEntry({
    id: "extract-audio-completed",
    tool: "Extract Audio",
    sourceFileName: "concert \u00e9t\u00e9 \u0639\u0631\u0628\u064a.mp4",
    resultPath: "C:\\Audio avec espaces\\concert \u00e9t\u00e9 \u0639\u0631\u0628\u064a.mp3",
    success: true,
    originalSize: 12_000,
    finalSize: 1_500,
    durationMs: 340,
    details: "Final audio format: MP3",
  });

  assert.equal(entry.category, "Video");
  assert.equal(entry.tool, "Extract Audio");
  assert.equal(entry.status, "Completed");
  assert.equal(entry.createdFileName, "concert \u00e9t\u00e9 \u0639\u0631\u0628\u064a.mp3");
  assert.equal(entry.originalSize, 12_000);
  assert.equal(entry.finalSize, 1_500);
  assert.equal(entry.durationMs, 340);
  assert.match(entry.details, /MP3/);
});

test("Extract Audio batches continue after no-track and FFmpeg failures without duplicates", async () => {
  const completed = createVideoProcessingHistoryEntry({
    id: "extract-batch-completed",
    tool: "Extract Audio",
    sourceFileName: "avec audio.mp4",
    resultPath: "C:\\Audio\\avec audio.flac",
    success: true,
    originalSize: 8_000,
    finalSize: 2_000,
    durationMs: 90,
    details: "Final audio format: FLAC",
  });
  const noTrack = createVideoProcessingHistoryEntry({
    id: "extract-batch-no-track",
    tool: "Extract Audio",
    sourceFileName: "silencieuse.mp4",
    success: false,
    originalSize: 7_000,
    durationMs: 4,
    errorMessage: "no_audio_stream",
  });
  const ffmpegFailure = createVideoProcessingHistoryEntry({
    id: "extract-batch-ffmpeg",
    tool: "Extract Audio",
    sourceFileName: "endommag\u00e9e.mov",
    success: false,
    originalSize: 6_000,
    durationMs: 12,
    errorMessage: "extraction_failed",
  });

  const stored = await appendProcessingHistoryEntries([
    noTrack,
    completed,
    ffmpegFailure,
    completed,
  ]);

  assert.equal(stored.length, 3);
  assert.deepEqual(stored.map((entry) => entry.status), ["Failed", "Completed", "Failed"]);
  assert.equal(stored[0].createdFilePath, "");
  assert.equal(stored[2].createdFilePath, "");
});

test("Extract Audio cancellation never retains a temporary or deleted result", () => {
  const entry = createVideoProcessingHistoryEntry({
    id: "extract-audio-cancelled",
    tool: "Extract Audio",
    sourceFileName: "longue vid\u00e9o.mp4",
    resultPath: "C:\\Audio\\.longue.filenova.tmp.ogg",
    success: false,
    cancelled: true,
    originalSize: 20_000,
    finalSize: 500,
    durationMs: 21,
    errorMessage: "extraction_cancelled",
  });

  assert.equal(entry.status, "Cancelled");
  assert.equal(entry.createdFileName, "");
  assert.equal(entry.createdFilePath, "");
  assert.equal(entry.finalSize, undefined);
});

test("Trim Video records successful Fast Cut and Precise Cut details", async () => {
  const fast = createVideoProcessingHistoryEntry({
    id: "trim-fast-completed",
    tool: "Trim Video",
    sourceFileName: "source \u00e9t\u00e9.mp4",
    resultPath: "C:\\Vid\u00e9os d\u00e9coup\u00e9es\\source \u00e9t\u00e9-trimmed.mp4",
    success: true,
    originalSize: 10_000,
    finalSize: 4_000,
    durationMs: 80,
    details: "Mode: Fast Cut \u00b7 Start: 00:00:01.000 \u00b7 End: 00:00:05.000 \u00b7 Selected duration: 00:00:04.000",
  });
  const precise = createVideoProcessingHistoryEntry({
    id: "trim-precise-completed",
    tool: "Trim Video",
    sourceFileName: "\u0641\u064a\u062f\u064a\u0648 \u0646\u0647\u0627\u0626\u064a.mkv",
    resultPath: "C:\\Vid\u00e9os d\u00e9coup\u00e9es\\\u0641\u064a\u062f\u064a\u0648 \u0646\u0647\u0627\u0626\u064a-precise-trim.mkv",
    success: true,
    originalSize: 15_000,
    finalSize: 6_000,
    durationMs: 180,
    details: "Mode: Precise Cut \u00b7 Start: 00:00:02.000 \u00b7 End: 00:00:08.000 \u00b7 Selected duration: 00:00:06.000",
  });

  const stored = await appendProcessingHistoryEntries([fast, precise]);
  assert.equal(stored.length, 2);
  assert.ok(stored.every((entry) => entry.status === "Completed"));
  assert.match(stored[0].details, /Fast Cut/);
  assert.match(stored[1].details, /Precise Cut/);
  assert.deepEqual(stored.map((entry) => entry.durationMs), [80, 180]);
});

test("Trim Video invalid range, FFmpeg failure and cancellation have no output", () => {
  const invalid = createVideoProcessingHistoryEntry({
    id: "trim-invalid-range",
    tool: "Trim Video",
    sourceFileName: "source.mp4",
    success: false,
    originalSize: 1_000,
    durationMs: 0,
    errorMessage: "trimStartBeforeEnd",
    details: "Mode: Fast Cut \u00b7 Start: 00:00:05.000 \u00b7 End: 00:00:02.000",
  });
  const failed = createVideoProcessingHistoryEntry({
    id: "trim-ffmpeg-failed",
    tool: "Trim Video",
    sourceFileName: "source.mp4",
    resultPath: "C:\\Sorties\\.source.filenova.tmp.mp4",
    success: false,
    originalSize: 1_000,
    finalSize: 100,
    durationMs: 15,
    errorMessage: "trim_failed",
  });
  const cancelled = createVideoProcessingHistoryEntry({
    id: "trim-cancelled",
    tool: "Trim Video",
    sourceFileName: "source.mp4",
    resultPath: "C:\\Sorties\\.source.cancelled.tmp.mp4",
    success: false,
    cancelled: true,
    originalSize: 1_000,
    finalSize: 100,
    durationMs: 10,
    errorMessage: "trim_cancelled",
  });

  assert.deepEqual([invalid.status, failed.status, cancelled.status], ["Failed", "Failed", "Cancelled"]);
  assert.ok([invalid, failed, cancelled].every((entry) => entry.createdFilePath === ""));
  assert.ok([invalid, failed, cancelled].every((entry) => entry.finalSize === undefined));
});

test("Extract Audio and Trim Video respect disabled/failing history and emit immediate updates", async () => {
  const snapshots = [];
  const listener = (event) => snapshots.push(event.detail.entries.map((entry) => entry.tool));
  window.addEventListener(PROCESSING_HISTORY_CHANGED_EVENT, listener);
  const extract = createVideoProcessingHistoryEntry({
    id: "extract-disabled",
    tool: "Extract Audio",
    sourceFileName: "source.mp4",
    resultPath: "C:\\Audio\\source.wav",
    success: true,
  });
  const trim = createVideoProcessingHistoryEntry({
    id: "trim-event",
    tool: "Trim Video",
    sourceFileName: "source.mp4",
    resultPath: "C:\\Video\\source-trimmed.mp4",
    success: true,
  });

  await appendProcessingHistoryEntry(extract, false);
  await appendProcessingHistoryEntry(trim, true);
  storage.failWrite = true;
  await assert.doesNotReject(appendProcessingHistoryEntry(extract, true));
  storage.failWrite = false;
  window.removeEventListener(PROCESSING_HISTORY_CHANGED_EVENT, listener);

  assert.deepEqual((await loadProcessingHistory()).map((entry) => entry.tool), ["Trim Video"]);
  assert.deepEqual(snapshots, [["Trim Video"]]);
});

test("Extract Audio and Trim Video history leave Unicode originals intact", async () => {
  const directory = await mkdtemp(join(tmpdir(), "filenova audio trim "));
  const sourcePath = join(directory, "vid\u00e9o \u0639\u0631\u0628\u064a \u4f60\u597d.mp4");
  const original = Buffer.from([1, 3, 5, 7, 9]);
  try {
    await writeFile(sourcePath, original);
    await appendProcessingHistoryEntries([
      createVideoProcessingHistoryEntry({
        tool: "Extract Audio",
        sourceFileName: sourcePath,
        resultPath: join(directory, "audio.mp3"),
        success: true,
      }),
      createVideoProcessingHistoryEntry({
        tool: "Trim Video",
        sourceFileName: sourcePath,
        resultPath: join(directory, "trimmed.mp4"),
        success: true,
      }),
    ]);
    assert.deepEqual(await readFile(sourcePath), original);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("Extract Audio and Trim Video pages connect canonical history without awaiting storage", async () => {
  const [extractSource, trimSource] = await Promise.all([
    readFile(new URL("../src/pages/ExtractAudioPage.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/pages/TrimVideoPage.tsx", import.meta.url), "utf8"),
  ]);

  assert.match(extractSource, /tool: "Extract Audio"/);
  assert.match(extractSource, /void appendProcessingHistoryEntries/);
  assert.match(extractSource, /no_audio_stream/);
  assert.match(extractSource, /settings\.enableProcessingHistory/);
  assert.match(trimSource, /tool: "Trim Video"/);
  assert.match(trimSource, /Mode: \$\{mode === "fast" \? "Fast Cut" : "Precise Cut"\}/);
  assert.match(trimSource, /cancelled = parsed\.code === "trim_cancelled"/);
  assert.match(trimSource, /void appendProcessingHistoryEntry/);
});

test("Video constructor supports all six stable canonical Video tool names", () => {
  const tools = [
    "Compress Video",
    "Convert Video",
    "Extract Audio",
    "Trim Video",
    "Video to GIF",
    "Social Media Tools",
  ];
  const entries = tools.map((tool, index) => createVideoProcessingHistoryEntry({
    id: `canonical-video-${index}`,
    tool,
    sourceFileName: "source.mp4",
    resultPath: `C:\\Video\\result-${index}.mp4`,
    success: true,
  }));

  assert.deepEqual(entries.map((entry) => entry.tool), tools);
  assert.ok(entries.every((entry) => entry.category === "Video"));
});

test("Video to GIF Completed records final path, sizes, duration, dimensions, fps and selection", () => {
  const entry = createVideoProcessingHistoryEntry({
    id: "gif-completed",
    tool: "Video to GIF",
    sourceFileName: "vid\u00e9o \u0639\u0631\u0628\u064a \u4f60\u597d.mp4",
    resultPath: "C:\\GIF avec espaces\\vid\u00e9o \u0639\u0631\u0628\u064a \u4f60\u597d-animation.gif",
    success: true,
    originalSize: 18_000,
    finalSize: 2_400,
    durationMs: 420,
    details: "Dimensions: 480x270 \u00b7 Frame rate: 15 fps \u00b7 Start: 00:00:01.000 \u00b7 Duration: 00:00:04.000",
  });

  assert.equal(entry.status, "Completed");
  assert.match(entry.createdFileName, /\.gif$/);
  assert.equal(entry.originalSize, 18_000);
  assert.equal(entry.finalSize, 2_400);
  assert.equal(entry.durationMs, 420);
  assert.match(entry.details, /480x270/);
  assert.match(entry.details, /15 fps/);
});

test("Video to GIF invalid parameters, FFmpeg error and cancellation never retain temporary outputs", () => {
  const invalid = createVideoProcessingHistoryEntry({
    id: "gif-invalid",
    tool: "Video to GIF",
    sourceFileName: "source.mp4",
    success: false,
    durationMs: -2,
    errorMessage: "gifInvalidFrameRate",
  });
  const failed = createVideoProcessingHistoryEntry({
    id: "gif-ffmpeg-failed",
    tool: "Video to GIF",
    sourceFileName: "source.mp4",
    resultPath: "C:\\GIF\\.palette.tmp.gif",
    success: false,
    originalSize: 9_000,
    finalSize: 100,
    durationMs: 15,
    errorMessage: "gif_creation_failed",
  });
  const cancelled = createVideoProcessingHistoryEntry({
    id: "gif-cancelled",
    tool: "Video to GIF",
    sourceFileName: "source.mp4",
    resultPath: "C:\\GIF\\.cancelled.tmp.gif",
    success: false,
    cancelled: true,
    originalSize: 9_000,
    finalSize: 100,
    durationMs: 20,
    errorMessage: "gif_cancelled",
  });

  assert.deepEqual([invalid.status, failed.status, cancelled.status], ["Failed", "Failed", "Cancelled"]);
  assert.equal(invalid.durationMs, 0);
  assert.ok([invalid, failed, cancelled].every((entry) => entry.createdFilePath === ""));
  assert.ok([invalid, failed, cancelled].every((entry) => entry.finalSize === undefined));
});

test("Social Media Tools records TikTok Fit, Instagram Fill and YouTube Stretch exports as a batch", async () => {
  const exports = [
    {
      id: "social-tiktok",
      sourceFileName: "vertical source.mp4",
      resultPath: "C:\\Social\\vertical source-tiktok-vertical.mp4",
      details: "Platform: TikTok \u00b7 Preset: tiktok-vertical \u00b7 Format: MP4 \u00b7 Ratio: 9:16 \u00b7 Dimensions: 1080x1920 \u00b7 Mode: Fit",
    },
    {
      id: "social-instagram",
      sourceFileName: "carr\u00e9 \u00e9t\u00e9.mp4",
      resultPath: "C:\\Social\\carr\u00e9 \u00e9t\u00e9-instagram-square.mp4",
      details: "Platform: Instagram \u00b7 Preset: instagram-square \u00b7 Format: MP4 \u00b7 Ratio: 1:1 \u00b7 Dimensions: 1080x1080 \u00b7 Mode: Fill",
    },
    {
      id: "social-youtube",
      sourceFileName: "\u0641\u064a\u062f\u064a\u0648 paysage.mp4",
      resultPath: "C:\\Social\\\u0641\u064a\u062f\u064a\u0648 paysage-youtube-standard.mp4",
      details: "Platform: YouTube \u00b7 Preset: youtube-standard \u00b7 Format: MP4 \u00b7 Ratio: 16:9 \u00b7 Dimensions: 1920x1080 \u00b7 Mode: Stretch",
    },
  ].map((input, index) => createVideoProcessingHistoryEntry({
    ...input,
    tool: "Social Media Tools",
    success: true,
    originalSize: 20_000 + index,
    finalSize: 8_000 + index,
    durationMs: 300 + index,
  }));

  const stored = await appendProcessingHistoryEntries([...exports, exports[0]]);
  assert.equal(stored.length, 3);
  assert.deepEqual(stored.map((entry) => entry.details.match(/Mode: (Fit|Fill|Stretch)/)?.[1]), ["Fit", "Fill", "Stretch"]);
  assert.deepEqual(stored.map((entry) => entry.createdFileName), [
    "vertical source-tiktok-vertical.mp4",
    "carr\u00e9 \u00e9t\u00e9-instagram-square.mp4",
    "\u0641\u064a\u062f\u064a\u0648 paysage-youtube-standard.mp4",
  ]);
});

test("Social Media Tools batch continues after failure and cancellation without preview or temporary paths", async () => {
  const completed = createVideoProcessingHistoryEntry({
    id: "social-completed-after-failure",
    tool: "Social Media Tools",
    sourceFileName: "good.mp4",
    resultPath: "C:\\Social\\good-youtube-standard.mp4",
    success: true,
  });
  const failed = createVideoProcessingHistoryEntry({
    id: "social-failed",
    tool: "Social Media Tools",
    sourceFileName: "bad.mp4",
    resultPath: "blob:preview-safe-zone",
    success: false,
    errorMessage: "social_export_failed",
  });
  const cancelled = createVideoProcessingHistoryEntry({
    id: "social-cancelled",
    tool: "Social Media Tools",
    sourceFileName: "cancel.mp4",
    resultPath: "C:\\Social\\.cancel.tmp.mp4",
    success: false,
    cancelled: true,
    errorMessage: "social_export_cancelled",
  });

  const stored = await appendProcessingHistoryEntries([failed, completed, cancelled]);
  assert.deepEqual(stored.map((entry) => entry.status), ["Failed", "Completed", "Cancelled"]);
  assert.equal(stored[0].createdFilePath, "");
  assert.equal(stored[2].createdFilePath, "");
  assert.ok(stored.every((entry) => !entry.createdFilePath.includes("preview")));
  assert.ok(stored.every((entry) => !entry.createdFilePath.includes(".tmp")));
});

test("Video to GIF and Social Media Tools respect disabled/failing history and update immediately", async () => {
  const events = [];
  const listener = (event) => events.push(event.detail.entries.map((entry) => entry.tool));
  window.addEventListener(PROCESSING_HISTORY_CHANGED_EVENT, listener);
  const gif = createVideoProcessingHistoryEntry({
    id: "gif-disabled",
    tool: "Video to GIF",
    sourceFileName: "source.mp4",
    resultPath: "C:\\GIF\\source.gif",
    success: true,
  });
  const social = createVideoProcessingHistoryEntry({
    id: "social-event",
    tool: "Social Media Tools",
    sourceFileName: "source.mp4",
    resultPath: "C:\\Social\\source.mp4",
    success: true,
  });

  await appendProcessingHistoryEntry(gif, false);
  await appendProcessingHistoryEntries([social], true);
  storage.failWrite = true;
  await assert.doesNotReject(appendProcessingHistoryEntries([gif], true));
  storage.failWrite = false;
  window.removeEventListener(PROCESSING_HISTORY_CHANGED_EVENT, listener);

  assert.deepEqual((await loadProcessingHistory()).map((entry) => entry.tool), ["Social Media Tools"]);
  assert.deepEqual(events, [["Social Media Tools"]]);
});

test("Video to GIF and Social Media Tools history preserve Unicode originals", async () => {
  const directory = await mkdtemp(join(tmpdir(), "filenova gif social "));
  const sourcePath = join(directory, "original \u00e9t\u00e9 \u0639\u0631\u0628\u064a \u4f60\u597d.mp4");
  const original = Buffer.from([2, 4, 6, 8, 10]);
  try {
    await writeFile(sourcePath, original);
    await appendProcessingHistoryEntries([
      createVideoProcessingHistoryEntry({
        tool: "Video to GIF",
        sourceFileName: sourcePath,
        resultPath: join(directory, "animation.gif"),
        success: true,
      }),
      createVideoProcessingHistoryEntry({
        tool: "Social Media Tools",
        sourceFileName: sourcePath,
        resultPath: join(directory, "social.mp4"),
        success: true,
      }),
    ]);
    assert.deepEqual(await readFile(sourcePath), original);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("Video to GIF and Social Media Tools pages use canonical non-localized history wiring", async () => {
  const [gifSource, socialSource] = await Promise.all([
    readFile(new URL("../src/pages/VideoToGifPage.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/pages/SocialMediaToolsPage.tsx", import.meta.url), "utf8"),
  ]);

  assert.match(gifSource, /tool: "Video to GIF"/);
  assert.match(gifSource, /cancelled: code === "gif_cancelled"/);
  assert.match(gifSource, /void appendProcessingHistoryEntry/);
  assert.match(socialSource, /tool: "Social Media Tools"/);
  assert.match(socialSource, /void appendProcessingHistoryEntries/);
  assert.match(socialSource, /cancelled: code === "social_export_cancelled"/);
  assert.match(socialSource, /Platform: \$\{socialPlatformName\(platform\)\}/);
  assert.doesNotMatch(socialSource, /details:.*safeZone/);
});

test("removes one history entry, emits an update and never deletes the real result file", async () => {
  const directory = await mkdtemp(join(tmpdir(), "filenova remove history "));
  const resultPath = join(directory, "résultat réel.txt");
  const contents = Buffer.from("keep this user file");
  const snapshots = [];
  const listener = (event) => snapshots.push(event.detail.entries.map((entry) => entry.id));
  try {
    await writeFile(resultPath, contents);
    await appendProcessingHistoryEntries([
      completedEntry("remove-me", { resultPath }),
      completedEntry("keep-me"),
    ]);
    window.addEventListener(PROCESSING_HISTORY_CHANGED_EVENT, listener);

    const result = await removeProcessingHistoryEntry("remove-me");

    window.removeEventListener(PROCESSING_HISTORY_CHANGED_EVENT, listener);
    assert.equal(result.success, true);
    assert.deepEqual(result.entries.map((entry) => entry.id), ["keep-me"]);
    assert.deepEqual(await readFile(resultPath), contents);
    assert.deepEqual(snapshots, [["keep-me"]]);
  } finally {
    window.removeEventListener(PROCESSING_HISTORY_CHANGED_EVENT, listener);
    await rm(directory, { recursive: true, force: true });
  }
});

test("remove entry and clear history report localStorage errors without losing stored data", async () => {
  await appendProcessingHistoryEntry(completedEntry("protected-entry"));
  storage.failWrite = true;

  const removed = await removeProcessingHistoryEntry("protected-entry");
  const cleared = await clearProcessingHistoryWithResult();

  storage.failWrite = false;
  assert.equal(removed.success, false);
  assert.equal(cleared.success, false);
  assert.deepEqual((await loadProcessingHistory()).map((entry) => entry.id), ["protected-entry"]);
});

test("clear history removes only history data, emits immediately and reports a successful read", async () => {
  const directory = await mkdtemp(join(tmpdir(), "filenova clear history "));
  const resultPath = join(directory, "user-output.txt");
  const snapshots = [];
  const listener = (event) => snapshots.push(event.detail.entries.length);
  try {
    await writeFile(resultPath, "preserve");
    await appendProcessingHistoryEntry(completedEntry("clear-me", { resultPath }));
    window.addEventListener(PROCESSING_HISTORY_CHANGED_EVENT, listener);

    const cleared = await clearProcessingHistoryWithResult();
    const loaded = await loadProcessingHistoryWithResult();

    window.removeEventListener(PROCESSING_HISTORY_CHANGED_EVENT, listener);
    assert.deepEqual(cleared, { entries: [], success: true });
    assert.deepEqual(loaded, { entries: [], success: true });
    assert.equal((await readFile(resultPath, "utf8")), "preserve");
    assert.deepEqual(snapshots, [0]);
  } finally {
    window.removeEventListener(PROCESSING_HISTORY_CHANGED_EVENT, listener);
    await rm(directory, { recursive: true, force: true });
  }
});

test("history filtering, Clear Filters defaults and readable formatting cover long Unicode data", () => {
  const entries = [
    completedEntry("filter-image", {
      sourceFileName: "très long fichier été 你好.png",
      originalSize: 512,
      finalSize: 1_536,
      durationMs: 65_432,
    }),
    createVideoProcessingHistoryEntry({
      id: "filter-video",
      tool: "Trim Video",
      sourceFileName: "فيديو نهائي.mp4",
      resultPath: "C:\\Video\\فيديو نهائي-trimmed.mp4",
      success: true,
      details: "Mode: Precise Cut",
    }),
  ];

  assert.deepEqual(filterProcessingHistory(entries, {
    category: "Video",
    status: "Completed",
    search: "دقيق",
  }).map((entry) => entry.id), []);
  assert.deepEqual(filterProcessingHistory(entries, {
    category: "Video",
    status: "Completed",
    search: "Precise",
  }).map((entry) => entry.id), ["filter-video"]);
  assert.deepEqual(DEFAULT_PROCESSING_HISTORY_FILTERS, { category: "all", status: "all", search: "" });
  assert.equal(formatHistoryBytes(512), "512 B");
  assert.equal(formatHistoryBytes(1_536), "1.5 KB");
  assert.equal(formatHistoryDuration(65_432), "1 min 5 s");
  assert.match(formatHistoryDate("2026-07-23T10:20:30.000Z", "fr"), /2026/);
});

test("Open File is available only for Completed entries whose result still exists", () => {
  const completed = completedEntry("open-completed");
  const failed = completedEntry("open-failed", { success: false, resultPath: undefined });
  const cancelled = completedEntry("open-cancelled", { success: false, cancelled: true, resultPath: undefined });

  assert.equal(canOpenProcessingResult(completed, { fileExists: true, parentExists: true }), true);
  assert.equal(canOpenProcessingResult(completed, { fileExists: false, parentExists: true }), false);
  assert.equal(canOpenProcessingResult(failed, { fileExists: true, parentExists: true }), false);
  assert.equal(canOpenProcessingResult(cancelled, { fileExists: true, parentExists: true }), false);
});

test("Processing History page confirms destructive history actions and keeps refresh and filters local", async () => {
  const pageSource = await readFile(new URL("../src/pages/ProcessingHistoryPage.tsx", import.meta.url), "utf8");

  assert.match(pageSource, /window\.confirm\(t\("historyRemoveConfirm"/);
  assert.match(pageSource, /window\.confirm\(t\("historyClearConfirm"/);
  assert.match(pageSource, /removeProcessingHistoryEntry/);
  assert.match(pageSource, /clearProcessingHistoryWithResult/);
  assert.match(pageSource, /check_processing_result_availability/);
  assert.match(pageSource, /revealItemInDir/);
  assert.match(pageSource, /function clearFilters\(\)/);
  assert.match(pageSource, /void refresh\(true\)/);
  assert.doesNotMatch(pageSource, /removeFile|deleteFile|fs\.remove/);
});

test("Processing History translations exist in English, French and Arabic with RTL support", async () => {
  const [english, french, arabic, provider] = await Promise.all([
    readFile(new URL("../src/i18n/en.ts", import.meta.url), "utf8"),
    readFile(new URL("../src/i18n/fr.ts", import.meta.url), "utf8"),
    readFile(new URL("../src/i18n/ar.ts", import.meta.url), "utf8"),
    readFile(new URL("../src/i18n/index.tsx", import.meta.url), "utf8"),
  ]);
  const requiredKeys = [
    "historyOpenFile",
    "historyOpenFolder",
    "historyRemoveEntry",
    "historyRefresh",
    "historyClearFilters",
    "historyFileUnavailable",
    "historyOpenError",
    "historyRemoveError",
  ];

  for (const source of [english, french, arabic]) {
    for (const key of requiredKeys) assert.match(source, new RegExp(`${key}:`));
  }
  assert.match(provider, /language === "ar" \? "rtl" : "ltr"/);
});
