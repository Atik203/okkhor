import { beforeEach, describe, expect, test, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Toaster } from "sonner";
import KeyboardPage from "../pages/keyboard";
import {
  DEFAULT_GROUP_ORDER,
  EMOJI,
  GROUP_ORDER_KEY,
  KEY_GROUPS,
  MAX_FAVORITES,
  FAVORITES_KEY,
  loadFavorites,
  loadGroupOrder,
  moveGroup,
  pushRecent,
  resolveKeyText,
  saveFavorites,
  saveGroupOrder,
  searchEmoji,
  toggleFavorite,
} from "../lib/keyboard-data";

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
      <KeyboardPage readOnly={readOnly} />
    </QueryClientProvider>,
  );
}

describe("key grid shift layer", () => {
  test("shift affects digits, leaves letters on the base layer", () => {
    const digits = KEY_GROUPS.find((group) => group.id === "digits");
    expect(digits).toBeDefined();
    expect(resolveKeyText(digits!.keys[0], false)).toBe("০");
    expect(resolveKeyText(digits!.keys[0], true)).toBe("0");
    const vowels = KEY_GROUPS.find((group) => group.id === "vowels");
    expect(resolveKeyText(vowels!.keys[0], true)).toBe(vowels!.keys[0].base);
    const symbols = KEY_GROUPS.find((group) => group.id === "symbols");
    expect(resolveKeyText(symbols!.keys[0], true)).toBe("॥");
  });

  test("every group has keys and every key inserts non-empty Unicode text", () => {
    expect(KEY_GROUPS.length).toBeGreaterThanOrEqual(5);
    for (const group of KEY_GROUPS) {
      expect(group.keys.length).toBeGreaterThan(0);
      for (const key of group.keys) {
        expect(key.base.length).toBeGreaterThan(0);
        expect(resolveKeyText(key, false).length).toBeGreaterThan(0);
      }
    }
  });
});

describe("group order persistence (non-sensitive, versioned key)", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  test("defaults to the canonical order when nothing is stored", () => {
    expect(loadGroupOrder()).toEqual(DEFAULT_GROUP_ORDER);
  });

  test("corrupt storage falls back to defaults without throwing", () => {
    window.localStorage.setItem(GROUP_ORDER_KEY, "not-json{{");
    expect(loadGroupOrder()).toEqual(DEFAULT_GROUP_ORDER);
  });

  test("stored order is sanitized: unknown ids dropped, missing appended, dupes removed", () => {
    window.localStorage.setItem(
      GROUP_ORDER_KEY,
      JSON.stringify(["digits", "nope", "digits", "vowels"]),
    );
    const order = loadGroupOrder();
    expect(order).toEqual([
      "digits",
      "vowels",
      "consonants",
      "signs",
      "symbols",
      "conjuncts",
    ]);
  });

  test("moveGroup steps one position and ignores out-of-range moves", () => {
    const order = ["a", "b", "c"];
    expect(moveGroup(order, "b", -1)).toEqual(["b", "a", "c"]);
    expect(moveGroup(order, "b", 1)).toEqual(["a", "c", "b"]);
    expect(moveGroup(order, "a", -1)).toEqual(order);
    expect(moveGroup(order, "c", 1)).toEqual(order);
    expect(moveGroup(order, "missing", 1)).toEqual(order);
  });

  test("round-trip persists only group ids under the versioned key", () => {
    saveGroupOrder(["symbols", "digits"]);
    expect(loadGroupOrder()[0]).toBe("symbols");
    expect(loadGroupOrder()[1]).toBe("digits");
    // The key is versioned; the value carries ids only — no keystroke text.
    expect(GROUP_ORDER_KEY).toContain("okkhor.osk.v");
    const raw = window.localStorage.getItem(GROUP_ORDER_KEY) ?? "";
    const parsed: unknown = JSON.parse(raw);
    expect(parsed).toEqual(["symbols", "digits"]);
    expect(raw).not.toContain("অ");
    expect(Object.keys(window.localStorage).filter((k) => k.startsWith("okkhor."))).toEqual([
      GROUP_ORDER_KEY,
    ]);
  });
});

describe("favorites and session-only recents", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  test("toggleFavorite pins and unpins a key", () => {
    expect(toggleFavorite([], "অ")).toEqual(["অ"]);
    expect(toggleFavorite(["অ", "ক"], "অ")).toEqual(["ক"]);
  });

  test("favorites are capped and load tolerates garbage", () => {
    const many = Array.from({ length: MAX_FAVORITES + 10 }, (_, i) => `k${i}`);
    saveFavorites(many);
    expect(loadFavorites().length).toBe(MAX_FAVORITES);
    window.localStorage.setItem(FAVORITES_KEY, "42");
    expect(loadFavorites()).toEqual([]);
  });

  test("pushRecent dedupes, prepends, caps — and never touches localStorage", () => {
    let recents: string[] = [];
    recents = pushRecent(recents, "অ");
    recents = pushRecent(recents, "ক");
    recents = pushRecent(recents, "অ");
    expect(recents).toEqual(["অ", "ক"]);
    for (let i = 0; i < 30; i++) {
      recents = pushRecent(recents, `x${i}`);
    }
    expect(recents.length).toBeLessThanOrEqual(12);
    expect(window.localStorage.length).toBe(0);
  });
});

describe("bundled emoji search", () => {
  test("list is bundled (~120 entries) with English and Bengali keywords", () => {
    expect(EMOJI.length).toBeGreaterThanOrEqual(100);
    for (const entry of EMOJI) {
      expect(entry.ch.length).toBeGreaterThan(0);
      expect(entry.en.length).toBeGreaterThan(0);
      expect(entry.bn.length).toBeGreaterThan(0);
    }
  });

  test("empty query returns the head of the list", () => {
    expect(searchEmoji("")).toEqual(EMOJI.slice(0, 60));
  });

  test("matches English keywords", () => {
    const results = searchEmoji("cat");
    expect(results.some((entry) => entry.ch === "🐱")).toBe(true);
  });

  test("matches Bengali keywords", () => {
    const results = searchEmoji("হাসি");
    expect(results.some((entry) => entry.ch === "😂")).toBe(true);
    expect(results.length).toBeGreaterThan(1);
  });

  test("multi-token queries intersect; unknown queries return nothing", () => {
    const both = searchEmoji("cat dog");
    expect(both.length).toBe(0);
    expect(searchEmoji("zzzz-no-such-emoji").length).toBe(0);
  });
});

describe("keyboard page interaction", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    window.localStorage.clear();
  });

  function onlineMocks(inserted: string[]) {
    invokeMock.mockImplementation((command: string, args?: { text?: string }) => {
      if (command === "ipc_list_layouts") {
        return Promise.resolve([
          { code: "avro", name: "Avro" },
          { code: "bijoy", name: "Bijoy" },
        ]);
      }
      if (command === "ipc_get_settings") {
        return Promise.resolve({ layout: "avro" });
      }
      if (command === "ipc_insert_text") {
        inserted.push(args?.text ?? "");
        return Promise.resolve(true);
      }
      return Promise.reject({ code: "daemon_unavailable", message: "down" });
    });
  }

  test("clicking a key calls ipc_insert_text with the exact Unicode payload", async () => {
    const inserted: string[] = [];
    onlineMocks(inserted);
    renderPage(false);
    const key = await screen.findByRole("button", { name: "Insert অ" });
    fireEvent.click(key);
    await waitFor(() => expect(inserted).toEqual(["অ"]));
    // Success shows no toast and records a session-only recent.
    expect(await screen.findByRole("button", { name: "Insert recent অ" })).toBeInTheDocument();
    expect(window.localStorage.getItem(GROUP_ORDER_KEY)).toBeNull();
  });

  test("shift toggle switches the grid to the shift layer", async () => {
    onlineMocks([]);
    renderPage(false);
    await screen.findByRole("button", { name: "Insert অ" });
    expect(screen.queryByRole("button", { name: "Insert 0" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Shift: off" }));
    expect(await screen.findByRole("button", { name: "Shift: on" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Insert 0" })).toBeInTheDocument();
  });

  test("pinning adds a favorite and persists only the pinned key", async () => {
    onlineMocks([]);
    renderPage(false);
    await screen.findByRole("button", { name: "Insert অ" });
    fireEvent.click(screen.getByRole("button", { name: "Pin অ to favorites" }));
    expect(await screen.findByText("Pinned keys. Stored on this machine only.")).toBeInTheDocument();
    const stored = JSON.parse(window.localStorage.getItem(FAVORITES_KEY) ?? "[]");
    expect(stored).toEqual(["অ"]);
  });

  test("insert failure shows a toast and only a toast", async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === "ipc_list_layouts") {
        return Promise.resolve([{ code: "avro", name: "Avro" }]);
      }
      if (command === "ipc_get_settings") {
        return Promise.resolve({ layout: "avro" });
      }
      if (command === "ipc_insert_text") {
        return Promise.reject({ code: "TextTooLong", message: "text exceeds the insert limit" });
      }
      return Promise.reject({ code: "daemon_unavailable", message: "down" });
    });
    renderPage(false);
    fireEvent.click(await screen.findByRole("button", { name: "Insert অ" }));
    expect(await screen.findByText("That text exceeds the insert limit.")).toBeInTheDocument();
  });

  test("daemon layouts feed the selector; Bijoy note names Settings", async () => {
    onlineMocks([]);
    renderPage(false);
    await waitFor(() => {
      expect(screen.getByText(/From the daemon's layouts\.list\./)).toBeInTheDocument();
    });
    expect(
      screen.getByText(/choose Output encoding → Bijoy in Settings/),
    ).toBeInTheDocument();
  });

  test("unavailable daemon disables keys and explains why", async () => {
    invokeMock.mockRejectedValue({ code: "daemon_unavailable", message: "down" });
    renderPage(true);
    expect(
      await screen.findByText(/The daemon is not running, so keys are disabled\./),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Insert অ" })).toBeDisabled();
    expect(screen.getByText(/showing the bundled list/)).toBeInTheDocument();
  });

  test("group reorder via keyboard buttons persists the new order", async () => {
    onlineMocks([]);
    renderPage(false);
    await screen.findByRole("button", { name: "Insert অ" });
    fireEvent.click(screen.getByRole("button", { name: "Move Vowels down" }));
    const stored = JSON.parse(window.localStorage.getItem(GROUP_ORDER_KEY) ?? "[]");
    expect(stored.indexOf("vowels")).toBeGreaterThan(stored.indexOf("consonants"));
  });

  test("emoji search filters the bundled list and inserts on click", async () => {
    const inserted: string[] = [];
    onlineMocks(inserted);
    renderPage(false);
    await screen.findByRole("button", { name: "Insert অ" });
    fireEvent.change(screen.getByLabelText("Search emoji"), { target: { value: "cat" } });
    const cat = await screen.findByRole("button", { name: /🐱/ });
    fireEvent.click(cat);
    await waitFor(() => expect(inserted).toEqual(["🐱"]));
  });
});
