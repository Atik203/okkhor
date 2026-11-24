import { describe, expect, test, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Toaster } from "sonner";
import App from "../app";
import StatusPage from "../pages/status";
import SettingsPage from "../pages/settings";
import KeyboardPage from "../pages/keyboard";
import WordsPage from "../pages/words";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

function renderWithClient(node: React.ReactNode) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <Toaster />
      {node}
    </QueryClientProvider>,
  );
}

const SETTINGS = {
  language: "bn",
  layout: "avro",
  input_mode: "bangla",
  output_encoding: "unicode",
  theme: "dark",
  accent: "teal",
  font_scale: 1,
  font_unicode: "kalpurush",
  font_bijoy: "sutonnymj",
  font_family: "",
  popup_enabled: true,
  popup_max_items: 5,
  toggle_hotkey: "F9",
  pause_hotkey: "Ctrl+Alt+Shift+P",
  autostart: false,
  learning_enabled: true,
  quickbar_visible: true,
  quickbar_collapsed: false,
  quickbar_x: null,
  quickbar_y: null,
  quickbar_opacity: 1,
  show_toggle_toast: true,
  word_manager_page_size: 50,
  ai_enabled: false,
  speech_enabled: false,
};

const STATUS = {
  daemon_version: "0.5.0",
  paused: false,
  language: "bn",
  layout: "avro",
  input_mode: "bangla",
  output_encoding: "unicode",
  popup_visible: false,
  words_total: 0,
  learning_enabled: true,
  autostart: false,
};

function online(command: string) {
  if (command === "ipc_status") {
    return Promise.resolve(STATUS);
  }
  if (command === "ipc_get_settings") {
    return Promise.resolve(SETTINGS);
  }
  if (command === "ipc_list_words") {
    return Promise.resolve({ total: 0, words: [] });
  }
  if (command === "ipc_list_layouts") {
    return Promise.resolve([{ code: "avro", name: "Avro" }]);
  }
  if (command === "ipc_ai_status") {
    return Promise.resolve({
      enabled: false,
      provider: "disabled",
      key_present: false,
      breaker_state: "closed",
      active_jobs: 0,
    });
  }
  return Promise.reject({ code: "daemon_unavailable", message: "down" });
}

function byTestId(id: string): HTMLElement {
  const node = document.querySelector(`[data-testid="${id}"]`);
  expect(node).not.toBeNull();
  return node as HTMLElement;
}

describe("responsive app shell (docs/ui.md layout rules)", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockImplementation(online);
  });

  test("sidebar is a fixed 240px rail and content is a centered max-w-6xl shell", async () => {
    window.location.hash = "#/about";
    renderWithClient(<App />);
    await waitFor(() => {
      expect(screen.getByText("About Okkhor")).toBeInTheDocument();
    });
    const nav = screen.getByRole("navigation", { name: "Primary" });
    expect(nav.className).toMatch(/w-60/);
    expect(nav.className).toMatch(/shrink-0/);
    const main = document.querySelector("main");
    expect(main).not.toBeNull();
    const shell = main!.firstElementChild as HTMLElement;
    expect(shell.className).toMatch(/max-w-6xl/);
    expect(shell.className).toMatch(/mx-auto/);
    expect(shell.className).toMatch(/w-full/);
  });

  test("status page uses a responsive card grid with shrink-safe children", async () => {
    renderWithClient(<StatusPage readOnly={false} />);
    await waitFor(() => {
      expect(screen.getByText("Words and startup")).toBeInTheDocument();
    });
    const root = byTestId("page-status");
    expect(root.className).toMatch(/md:grid-cols-2/);
    expect(root.className).toMatch(/min-w-0/);
    expect(root.className).toMatch(/\[\&\>\*\]:min-w-0/);
  });

  test("settings form groups into two columns at lg+", async () => {
    renderWithClient(<SettingsPage readOnly={false} />);
    await waitFor(() => {
      expect(screen.getByText("Language and output")).toBeInTheDocument();
    });
    const form = byTestId("page-settings");
    expect(form.className).toMatch(/lg:grid-cols-2/);
    expect(form.className).toMatch(/min-w-0/);
  });

  test("words page is a two-column layout at xl with the table on the left", async () => {
    renderWithClient(<WordsPage readOnly={false} />);
    await waitFor(() => {
      expect(screen.getByText("Word manager")).toBeInTheDocument();
    });
    const root = byTestId("page-words");
    expect(root.className).toMatch(/xl:grid-cols-5/);
    const tableCard = root.firstElementChild as HTMLElement;
    expect(tableCard.className).toMatch(/xl:col-span-3/);
    const rail = root.children.item(1) as HTMLElement;
    expect(rail.className).toMatch(/xl:col-span-2/);
    // Add + import + export stack in the rail.
    expect(rail.textContent).toMatch(/Add a word/);
    expect(rail.textContent).toMatch(/Import words/);
    expect(rail.textContent).toMatch(/Export words/);
  });

  test("keyboard groups reflow in a responsive grid", async () => {
    renderWithClient(<KeyboardPage readOnly={false} />);
    await waitFor(() => {
      expect(screen.getByText("On-screen keyboard")).toBeInTheDocument();
    });
    const root = byTestId("page-keyboard");
    expect(root.className).toMatch(/min-w-0/);
    const groupsGrid = root.querySelector('[aria-label^="Key group:"]')?.parentElement;
    expect(groupsGrid).not.toBeNull();
    expect(groupsGrid!.className).toMatch(/lg:grid-cols-2/);
    expect(groupsGrid!.className).toMatch(/min-w-0/);
  });
});
