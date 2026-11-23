import { beforeEach, describe, expect, test, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Toaster } from "sonner";
import WordsPage from "../pages/words";
import { describeError } from "../lib/errors";
import {
  clampPage,
  previewCsvImport,
  previewImport,
  previewJsonImport,
  totalPages,
  wordFormSchema,
} from "../lib/words";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

function renderPage(readOnly: boolean) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <Toaster />
      <WordsPage readOnly={readOnly} />
    </QueryClientProvider>,
  );
}

function onlineWords(total: number, words: { id: number }[] = []) {
  return {
    total,
    words: words.map((entry, index) => ({
      id: entry.id,
      roman: `r${index}`,
      output: `o${index}`,
      language: "bn" as const,
      source: "user" as const,
      added_at_ms: 1,
    })),
  };
}

describe("word form validation mirrors contract §3", () => {
  test("valid Bangla and English words pass", () => {
    expect(
      wordFormSchema.safeParse({ roman: "amar", output: "আমার", language: "bn" }).success,
    ).toBe(true);
    expect(
      wordFormSchema.safeParse({ roman: "hello", output: "hello", language: "en" }).success,
    ).toBe(true);
  });

  test("empty and over-long fields fail", () => {
    expect(
      wordFormSchema.safeParse({ roman: "", output: "x", language: "bn" }).success,
    ).toBe(false);
    expect(
      wordFormSchema.safeParse({ roman: "a".repeat(64), output: "x", language: "bn" }).success,
    ).toBe(true);
    expect(
      wordFormSchema.safeParse({ roman: "a".repeat(65), output: "x", language: "bn" }).success,
    ).toBe(false);
    expect(
      wordFormSchema.safeParse({ roman: "a", output: "অ".repeat(128), language: "bn" }).success,
    ).toBe(true);
    expect(
      wordFormSchema.safeParse({ roman: "a", output: "অ".repeat(129), language: "bn" }).success,
    ).toBe(false);
  });

  test("control characters fail; ZWJ/ZWNJ pass", () => {
    const zwj = String.fromCharCode(0x200d);
    const zwnj = String.fromCharCode(0x200c);
    expect(
      wordFormSchema.safeParse({ roman: "a\tb", output: "x", language: "bn" }).success,
    ).toBe(false);
    expect(
      wordFormSchema.safeParse({ roman: "a", output: `ক্${zwj}ষ`, language: "bn" }).success,
    ).toBe(true);
    expect(
      wordFormSchema.safeParse({ roman: "a", output: `ক্${zwnj}ষ`, language: "bn" }).success,
    ).toBe(true);
  });

  test("unknown languages fail", () => {
    expect(
      wordFormSchema.safeParse({ roman: "a", output: "x", language: "xx" }).success,
    ).toBe(false);
  });
});

describe("import preview parsing", () => {
  test("CSV preview shows the first rows with imported/skipped counts", () => {
    const rows = Array.from(
      { length: 25 },
      (_, i) => `r${i},o${i},${i % 2 === 0 ? "bn" : "en"}`,
    ).join("\r\n");
    const preview = previewCsvImport(`roman,output,language\r\n${rows}\r\n`);
    expect(preview.fatal).toBeNull();
    expect(preview.rows.length).toBe(20);
    expect(preview.totalRows).toBe(25);
    expect(preview.imported).toBe(25);
    expect(preview.skipped).toBe(0);
  });

  test("CSV preview flags invalid rows and intra-file duplicates as skipped", () => {
    const preview = previewCsvImport(
      "roman,output,language\r\namar,আমার,bn\r\namar,আমার,bn\r\nbad,,xx\r\n\r\nvalo,ভালো,bn\r\n",
    );
    expect(preview.fatal).toBeNull();
    expect(preview.imported).toBe(2);
    expect(preview.skipped).toBe(2);
    expect(preview.rows[1].error).toContain("Duplicate");
  });

  test("CSV preview rejects a bad header and the over-cap payload", () => {
    expect(previewCsvImport("roman,output\r\na,b,bn\r\n").fatal).toContain("header");
    expect(previewCsvImport("not csv at all\njust one line").fatal).not.toBeNull();
    const big = `roman,output,language\r\n${Array.from({ length: 5001 }, (_, i) => `r${i},o${i},bn`).join("\r\n")}\r\n`;
    const over = previewCsvImport(big);
    expect(over.fatal).toContain("5,000");
    expect(over.imported).toBe(0);
  });

  test("JSON preview handles envelopes and bare arrays", () => {
    const envelope = previewJsonImport(
      '{"schema":1,"words":[{"roman":"amar","output":"আমার","language":"bn"}]}',
    );
    expect(envelope.fatal).toBeNull();
    expect(envelope.imported).toBe(1);
    const array = previewJsonImport('[{"roman":"ami","output":"আমি","language":"bn"}]');
    expect(array.imported).toBe(1);
    expect(previewJsonImport('{"schema":2,"words":[]}').fatal).toContain("schema");
    expect(previewJsonImport("nope").fatal).toContain("Malformed");
  });

  test("previewImport dispatches on format", () => {
    const csv = previewImport("roman,output,language\r\na,b,bn\r\n", "csv");
    expect(csv.imported).toBe(1);
    const json = previewImport('[{"roman":"a","output":"b","language":"bn"}]', "json");
    expect(json.imported).toBe(1);
  });
});

describe("pagination helpers", () => {
  test("totalPages guards zero sizes and rounds up", () => {
    expect(totalPages(0, 50)).toBe(1);
    expect(totalPages(50, 50)).toBe(1);
    expect(totalPages(51, 50)).toBe(2);
    expect(totalPages(120, 50)).toBe(3);
    expect(totalPages(10, 0)).toBe(1);
  });

  test("clampPage pins out-of-range pages", () => {
    expect(clampPage(0, 120, 50)).toBe(0);
    expect(clampPage(9, 120, 50)).toBe(2);
    expect(clampPage(-1, 120, 50)).toBe(0);
    expect(clampPage(0, 0, 50)).toBe(0);
  });
});

describe("daemon error text stays static (issue #72)", () => {
  test("word codes map to static text without echoing the payload", () => {
    const hostile = { code: "DuplicateWord", message: "dupe: আমার hunter2" };
    const text = describeError(hostile);
    expect(text).toContain("already exists");
    expect(text).not.toContain("আমার");
    expect(text).not.toContain("hunter2");
  });
});

describe("words page interaction", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  function mocks(state: {
    words?: { id: number }[];
    total?: number;
    calls?: { command: string; args?: Record<string, unknown> }[];
  }) {
    const total = state.total ?? state.words?.length ?? 0;
    invokeMock.mockImplementation((command: string, args?: Record<string, unknown>) => {
      state.calls?.push({ command, args });
      if (command === "ipc_get_settings") {
        return Promise.resolve({ word_manager_page_size: 50 });
      }
      if (command === "ipc_list_words") {
        return Promise.resolve(onlineWords(total, state.words ?? []));
      }
      if (command === "ipc_add_word") {
        return Promise.reject({ code: "DuplicateWord", message: "duplicate word" });
      }
      return Promise.reject({ code: "daemon_unavailable", message: "down" });
    });
  }

  test("paginated table renders rows, counts, and pages", async () => {
    const calls: { command: string; args?: Record<string, unknown> }[] = [];
    mocks({ words: [{ id: 7 }], total: 1, calls });
    renderPage(false);
    expect(await screen.findByText("1 word stored")).toBeInTheDocument();
    expect(screen.getByText("Page 1 of 1")).toBeInTheDocument();
    expect(
      invokeMock.mock.calls.some(
        ([command, args]) =>
          command === "ipc_list_words" &&
          (args as { limit: number }).limit === 50 &&
          (args as { offset: number }).offset === 0,
      ),
    ).toBe(true);
  });

  test("search submits the query and resets to page one", async () => {
    mocks({ words: [], total: 0 });
    renderPage(false);
    await screen.findByText("0 words stored");
    fireEvent.change(screen.getByLabelText("Search"), { target: { value: "amar" } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    await waitFor(() => {
      expect(
        invokeMock.mock.calls.some(
          ([command, args]) =>
            command === "ipc_list_words" && (args as { query: string }).query === "amar",
        ),
      ).toBe(true);
    });
  });

  test("next page requests the following offset", async () => {
    mocks({ words: [{ id: 1 }], total: 120 });
    renderPage(false);
    await screen.findByText("120 words stored");
    expect(screen.getByText("Page 1 of 3")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Next word page" }));
    await waitFor(() => {
      expect(
        invokeMock.mock.calls.some(
          ([command, args]) =>
            command === "ipc_list_words" && (args as { offset: number }).offset === 50,
        ),
      ).toBe(true);
    });
  });

  test("add form shows client validation inline", async () => {
    mocks({ words: [], total: 0 });
    renderPage(false);
    await screen.findByText("0 words stored");
    fireEvent.click(screen.getByRole("button", { name: "Add word" }));
    expect(await screen.findByText("Roman text is required.")).toBeInTheDocument();
  });

  test("daemon duplicate error renders inline without echoing the word", async () => {
    mocks({ words: [], total: 0 });
    renderPage(false);
    await screen.findByText("0 words stored");
    fireEvent.change(screen.getByLabelText(/Roman/), { target: { value: "amar" } });
    fireEvent.change(screen.getByLabelText(/Output/), { target: { value: "আমার" } });
    fireEvent.click(screen.getByRole("button", { name: "Add word" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("already exists");
    expect(alert.textContent).not.toContain("আমার");
  });

  test("remove asks for confirmation, then calls ipc_remove_word", async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === "ipc_get_settings") {
        return Promise.resolve({ word_manager_page_size: 50 });
      }
      if (command === "ipc_list_words") {
        return Promise.resolve(onlineWords(1, [{ id: 7 }]));
      }
      if (command === "ipc_remove_word") {
        return Promise.resolve(true);
      }
      return Promise.reject({ code: "daemon_unavailable", message: "down" });
    });
    renderPage(false);
    fireEvent.click(await screen.findByRole("button", { name: "Remove word 7" }));
    expect(await screen.findByText("Remove this word?")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() => {
      expect(
        invokeMock.mock.calls.some(
          ([command, args]) =>
            command === "ipc_remove_word" && (args as { id: number }).id === 7,
        ),
      ).toBe(true);
    });
  });

  test("import file upload previews the first rows with estimates", async () => {
    mocks({ words: [], total: 0 });
    renderPage(false);
    await screen.findByText("0 words stored");
    const file = new File(["roman,output,language\namar,আমার,bn\n"], "words.csv", {
      type: "text/csv",
    });
    fireEvent.change(screen.getByLabelText("File"), { target: { files: [file] } });
    expect(await screen.findByText(/1 new, 0 skipped \(estimated\)/)).toBeInTheDocument();
  });

  test("export downloads a blob without a dialog plugin", async () => {
    const create = vi.fn(() => "blob:mock");
    const revoke = vi.fn();
    const originalCreate = URL.createObjectURL;
    const originalRevoke = URL.revokeObjectURL;
    URL.createObjectURL = create;
    URL.revokeObjectURL = revoke;
    try {
      invokeMock.mockImplementation((command: string) => {
        if (command === "ipc_get_settings") {
          return Promise.resolve({ word_manager_page_size: 50 });
        }
        if (command === "ipc_list_words") {
          return Promise.resolve(onlineWords(0, []));
        }
        if (command === "ipc_export_words") {
          return Promise.resolve({ format: "csv", data: "roman,output,language\n" });
        }
        return Promise.reject({ code: "daemon_unavailable", message: "down" });
      });
      renderPage(false);
      await screen.findByText("0 words stored");
      fireEvent.click(screen.getByRole("button", { name: "Download export" }));
      await waitFor(() => expect(create).toHaveBeenCalled());
    } finally {
      URL.createObjectURL = originalCreate;
      URL.revokeObjectURL = originalRevoke;
    }
  });

  test("unavailable daemon renders read-only with an explanation", async () => {
    invokeMock.mockRejectedValue({ code: "daemon_unavailable", message: "down" });
    renderPage(true);
    expect(
      await screen.findByText(/the word list is read-only/),
    ).toBeInTheDocument();
    // Table errors surface the static daemon message, never raw text.
    const alerts = await screen.findAllByRole("alert");
    expect(alerts.length).toBeGreaterThan(0);
    for (const alert of alerts) {
      expect(alert.textContent).not.toContain("down");
    }
  });

  test("table exposes row counts for screen readers", async () => {
    mocks({ words: [{ id: 1 }, { id: 2 }], total: 2 });
    renderPage(false);
    const caption = await screen.findByText("2 words stored");
    expect(within(caption.closest("table")!).getAllByRole("row").length).toBe(3);
  });
});
