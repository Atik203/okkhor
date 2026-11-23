import { z } from "zod";

/**
 * Word-manager client logic (0.5 Task 5, contract §3).
 *
 * Validation mirrors the daemon (`okkhor-ipc` words.rs): `roman` 1–64 Unicode
 * scalar values, `output` 1–128 scalar values, no control characters in either
 * field (`Cc`; ZWJ/ZWNJ are format characters and stay allowed), language
 * `bn`/`en`. The daemon re-validates authoritatively; the import preview here
 * is an estimate labeled as such in the UI.
 *
 * Privacy: these helpers never log word text. Errors are static strings.
 */

export interface Word {
  id: number;
  roman: string;
  output: string;
  language: "bn" | "en";
  source: "user" | "import" | "learned";
  added_at_ms: number;
}

export interface WordsPage {
  total: number;
  words: Word[];
}

export type ImportFormat = "csv" | "json";

/** Control characters (`Cc`) rejected in word fields; ZWJ/ZWNJ pass. */
function hasControlChar(text: string): boolean {
  for (const ch of text) {
    const cp = ch.codePointAt(0) ?? 0;
    if ((cp >= 0x00 && cp <= 0x1f) || (cp >= 0x7f && cp <= 0x9f)) {
      return true;
    }
  }
  return false;
}

/** Byte-order mark, via code point (never a raw literal in source). */
const BOM = String.fromCharCode(0xfeff);
/** Dedup separator: U+0001 cannot appear in validated rows. */
const KEY_SEP = String.fromCharCode(0x01);

function scalarLength(text: string): number {
  return Array.from(text).length;
}

/** Client-side mirror of the §3 field rules (daemon decides on submit). */
export const wordFormSchema = z.object({
  roman: z
    .string()
    .min(1, "Roman text is required.")
    .refine((value) => scalarLength(value) <= 64, {
      message: "Roman text must be at most 64 characters.",
    })
    .refine((value) => !hasControlChar(value), {
      message: "Roman text must not contain control characters.",
    }),
  output: z
    .string()
    .min(1, "Output text is required.")
    .refine((value) => scalarLength(value) <= 128, {
      message: "Output text must be at most 128 characters.",
    })
    .refine((value) => !hasControlChar(value), {
      message: "Output text must not contain control characters.",
    }),
  language: z.enum(["bn", "en"]),
});

export type WordForm = z.infer<typeof wordFormSchema>;

export interface PreviewRow {
  roman: string;
  output: string;
  language: string;
  /** Empty when the row looks importable; otherwise a static reason. */
  error: string | null;
}

export interface ImportPreview {
  /** First rows for display (capped). */
  rows: PreviewRow[];
  /** Total data rows seen (excluding the header / envelope overhead). */
  totalRows: number;
  /** Rows that look importable (deduplicated within the payload). */
  imported: number;
  /** Rows skipped: invalid, duplicate, or malformed. */
  skipped: number;
  /** Fatal problem (bad header, malformed payload, over cap); static text. */
  fatal: string | null;
}

/** Strip one leading BOM without a raw literal in source. */
function stripBom(text: string): string {
  return text.startsWith(BOM) ? text.slice(1) : text;
}

/** Rows shown in the preview table. */
export const PREVIEW_ROW_LIMIT = 20;
/** Import cap per request, mirroring `MAX_IMPORT_ROWS` (contract §3). */
export const MAX_IMPORT_ROWS = 5000;

function checkRow(roman: string, output: string, language: string): string | null {
  if (language !== "bn" && language !== "en") {
    return "Language must be bn or en.";
  }
  const parsed = wordFormSchema.safeParse({ roman, output, language });
  if (!parsed.success) {
    return parsed.error.issues[0]?.message ?? "Invalid row.";
  }
  return null;
}

function finishPreview(all: PreviewRow[]): ImportPreview {
  const seen = new Set<string>();
  let imported = 0;
  let skipped = 0;
  const rows = all.map((row) => {
    if (row.error !== null) {
      skipped += 1;
      return row;
    }
    const key = [row.language, row.roman, row.output].join(KEY_SEP);
    if (seen.has(key)) {
      skipped += 1;
      return { ...row, error: "Duplicate row in this file." };
    }
    seen.add(key);
    imported += 1;
    return row;
  });
  return {
    rows: rows.slice(0, PREVIEW_ROW_LIMIT),
    totalRows: all.length,
    imported,
    skipped,
    fatal: null,
  };
}

/** Minimal RFC-4180-ish CSV split: quotes, embedded commas/quotes, CRLF, BOM. */
function splitCsvRecords(text: string): string[][] | null {
  const stripped = stripBom(text);
  const chars = Array.from(stripped);
  const records: string[][] = [];
  let field = "";
  let record: string[] = [];
  let inQuotes = false;
  let index = 0;
  let hasData = false;
  let recordHasContent = false;
  const endField = () => {
    record.push(field);
    field = "";
  };
  const endRecord = () => {
    endField();
    if (record.every((cell) => cell.trim() === "")) {
      record = [];
      recordHasContent = false;
    } else if (recordHasContent) {
      records.push(record);
      record = [];
      recordHasContent = false;
    } else {
      record = [];
    }
  };
  while (index < chars.length) {
    const ch = chars[index];
    hasData = true;
    if (inQuotes) {
      if (ch === '"') {
        if (chars[index + 1] === '"') {
          field += '"';
          index += 1;
        } else {
          inQuotes = false;
        }
      } else {
        field += ch;
      }
    } else if (ch === '"') {
      if (field === "") {
        inQuotes = true;
      } else {
        return null;
      }
    } else if (ch === ",") {
      endField();
    } else if (ch === "\r") {
      if (chars[index + 1] === "\n") {
        index += 1;
      }
      endRecord();
    } else if (ch === "\n") {
      endRecord();
    } else {
      field += ch;
    }
    if (ch !== "\r" && ch !== "\n") {
      recordHasContent = true;
    }
    index += 1;
  }
  if (inQuotes) {
    return null;
  }
  if (hasData) {
    endField();
    if (recordHasContent && !record.every((cell) => cell.trim() === "")) {
      records.push(record);
    }
  }
  return records;
}

/** Parse a CSV payload into a display preview (first rows + counts). */
export function previewCsvImport(text: string): ImportPreview {
  const records = splitCsvRecords(text);
  if (records === null || records.length === 0) {
    return { rows: [], totalRows: 0, imported: 0, skipped: 0, fatal: "Malformed CSV payload." };
  }
  const [header, ...body] = records;
  if (header.length !== 3 || header[0] !== "roman" || header[1] !== "output" || header[2] !== "language") {
    return {
      rows: [],
      totalRows: 0,
      imported: 0,
      skipped: 0,
      fatal: "CSV header must be exactly: roman,output,language.",
    };
  }
  if (body.length > MAX_IMPORT_ROWS) {
    return {
      rows: [],
      totalRows: body.length,
      imported: 0,
      skipped: 0,
      fatal: "Import exceeds the 5,000-row per-request cap.",
    };
  }
  return finishPreview(
    body.map((cells) => {
      if (cells.length !== 3) {
        return { roman: "", output: "", language: "", error: "Row must have 3 columns." };
      }
      const [roman, output, language] = cells;
      return { roman, output, language: language.trim(), error: checkRow(roman, output, language.trim()) };
    }),
  );
}

/** Parse a JSON payload (envelope or bare array) into a display preview. */
export function previewJsonImport(text: string): ImportPreview {
  const stripped = stripBom(text).trim();
  let entries: unknown;
  try {
    const parsed: unknown = JSON.parse(stripped);
    if (typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)) {
      const envelope = parsed as { schema?: unknown; words?: unknown };
      if (envelope.schema !== 1 || !Array.isArray(envelope.words)) {
        return {
          rows: [],
          totalRows: 0,
          imported: 0,
          skipped: 0,
          fatal: "JSON must be {schema: 1, words: [...]} or a row array.",
        };
      }
      entries = envelope.words;
    } else if (Array.isArray(parsed)) {
      entries = parsed;
    } else {
      return { rows: [], totalRows: 0, imported: 0, skipped: 0, fatal: "Malformed JSON payload." };
    }
  } catch {
    return { rows: [], totalRows: 0, imported: 0, skipped: 0, fatal: "Malformed JSON payload." };
  }
  const list = entries as unknown[];
  if (list.length > MAX_IMPORT_ROWS) {
    return {
      rows: [],
      totalRows: list.length,
      imported: 0,
      skipped: 0,
      fatal: "Import exceeds the 5,000-row per-request cap.",
    };
  }
  return finishPreview(
    list.map((item) => {
      if (typeof item !== "object" || item === null) {
        return { roman: "", output: "", language: "", error: "Row must be an object." };
      }
      const row = item as { roman?: unknown; output?: unknown; language?: unknown };
      const roman = typeof row.roman === "string" ? row.roman : "";
      const output = typeof row.output === "string" ? row.output : "";
      const language = typeof row.language === "string" ? row.language : "";
      if (typeof row.roman !== "string" || typeof row.output !== "string") {
        return { roman, output, language, error: "Row needs roman and output text." };
      }
      return { roman, output, language, error: checkRow(roman, output, language) };
    }),
  );
}

export function previewImport(text: string, format: ImportFormat): ImportPreview {
  return format === "csv" ? previewCsvImport(text) : previewJsonImport(text);
}

/** Page count for a total, guarding against zero/negative sizes. */
export function totalPages(total: number, pageSize: number): number {
  if (pageSize <= 0) {
    return 1;
  }
  return Math.max(1, Math.ceil(total / pageSize));
}

/** Clamp a page index into range. Pure (testable). */
export function clampPage(page: number, total: number, pageSize: number): number {
  return Math.min(Math.max(0, page), totalPages(total, pageSize) - 1);
}

/** Trigger a client-side download of exported words (blob, no plugin). */
export function downloadTextFile(filename: string, text: string, mime: string): void {
  const blob = new Blob([text], { type: `${mime};charset=utf-8` });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}
