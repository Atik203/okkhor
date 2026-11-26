import { beforeEach, describe, expect, test, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import QuickbarPage from "../pages/quickbar";
import type { QuickbarSnapshot } from "../lib/ipc";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    outerPosition: () => Promise.resolve({ x: 0, y: 0 }),
    setPosition: () => Promise.resolve(),
    setSize: () => Promise.resolve(),
    scaleFactor: () => Promise.resolve(1),
  }),
  PhysicalPosition: class PhysicalPosition {
    constructor(
      public x: number,
      public y: number,
    ) {}
  },
  LogicalSize: class LogicalSize {
    constructor(
      public width: number,
      public height: number,
    ) {}
  },
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => undefined),
}));

const ONLINE: QuickbarSnapshot = {
  input_mode: "bangla",
  output_encoding: "unicode",
  paused: false,
  available: true,
  collapsed: false,
  opacity: 0.9,
  visible: true,
};

function renderPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <QuickbarPage />
    </QueryClientProvider>,
  );
}

function mockOnline(snapshot: QuickbarSnapshot = ONLINE) {
  let current = { ...snapshot };
  invokeMock.mockImplementation((command: string) => {
    switch (command) {
      case "quickbar_state":
        return Promise.resolve({ ...current });
      case "quickbar_toggle_mode":
        current = {
          ...current,
          input_mode: current.input_mode === "bangla" ? "english" : "bangla",
        };
        return Promise.resolve({ ...current });
      case "quickbar_cycle_encoding":
        current = {
          ...current,
          output_encoding:
            current.output_encoding === "unicode" ? "bijoy" : "unicode",
        };
        return Promise.resolve({ ...current });
      case "quickbar_toggle_pause":
        current = { ...current, paused: !current.paused };
        return Promise.resolve({ ...current });
      case "quickbar_collapse":
        current = { ...current, collapsed: true };
        return Promise.resolve({ ...current });
      case "quickbar_expand":
        current = { ...current, collapsed: false };
        return Promise.resolve({ ...current });
      case "quickbar_open_settings":
        return Promise.resolve(undefined);
      case "quickbar_open_keyboard":
        return Promise.resolve(undefined);
      case "quickbar_hide":
      case "quickbar_show":
      case "quickbar_save_position":
      case "quickbar_begin_resize":
      case "quickbar_quit":
        return Promise.resolve(undefined);
      default:
        return Promise.reject({ code: "daemon_unavailable", message: "down" });
    }
  });
  return () => current;
}

describe("quick bar", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  test("keeps only the typing-surface controls", async () => {
    mockOnline();
    renderPage();
    expect(
      await screen.findByRole("button", { name: "Switch to English" }),
    ).toHaveTextContent("বাংলা");
    expect(
      screen.getByRole("button", { name: "Pause capture" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", {
        name: "Add the last typed word to my dictionary",
      }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Open settings" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Drag to move quick bar" }),
    ).toBeInTheDocument();
    // Everything rare lives in Settings; the bar is a typing surface, not a
    // control panel.
    expect(
      screen.queryByRole("button", { name: "Switch output to Bijoy" }),
    ).toBeNull();
    expect(
      screen.queryByRole("button", { name: "Open on-screen keyboard" }),
    ).toBeNull();
    expect(screen.queryByRole("button", { name: "Quit Okkhor" })).toBeNull();
    expect(
      screen.queryByRole("separator", { name: "Resize quick bar" }),
    ).toBeNull();
    // Status is text, not color alone.
    expect(screen.getByLabelText("Daemon connected")).toBeInTheDocument();
    expect(screen.getByText("Daemon connected.")).toBeInTheDocument();
    // Toolbar landmark, no user text captured.
    expect(
      screen.getByRole("toolbar", { name: "Okkhor quick bar" }),
    ).toBeInTheDocument();
  });

  test("mode chip toggles Bangla/English via quickbar_toggle_mode", async () => {
    mockOnline();
    renderPage();
    fireEvent.click(
      await screen.findByRole("button", { name: "Switch to English" }),
    );
    expect(
      await screen.findByRole("button", { name: "Switch to Bangla" }),
    ).toHaveTextContent("EN");
    expect(invokeMock).toHaveBeenCalledWith("quickbar_toggle_mode");
  });

  test("pause toggles capture with a toast", async () => {
    mockOnline();
    renderPage();
    fireEvent.click(await screen.findByRole("button", { name: "Pause capture" }));
    expect(
      await screen.findByRole("button", { name: "Resume capture" }),
    ).toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("quickbar_toggle_pause");
    expect(await screen.findByText("Capture paused.")).toBeInTheDocument();
  });

  test("double-clicking the grip collapses to the orb; clicking the orb expands", async () => {
    mockOnline();
    renderPage();
    fireEvent.doubleClick(
      await screen.findByRole("button", { name: "Drag to move quick bar" }),
    );
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("quickbar_collapse"),
    );
    const orb = await screen.findByRole("button", { name: "Expand quick bar" });
    expect(orb).toHaveTextContent("অ");
    fireEvent.click(orb);
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("quickbar_expand"),
    );
    expect(
      await screen.findByRole("button", { name: "Switch to English" }),
    ).toBeInTheDocument();
  });

  test("settings button opens the settings window", async () => {
    mockOnline();
    renderPage();
    fireEvent.click(
      await screen.findByRole("button", { name: "Open settings" }),
    );
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("quickbar_open_settings"),
    );
  });

  test("unavailable daemon disables mode actions but keeps settings local", async () => {
    mockOnline({ ...ONLINE, available: false });
    renderPage();
    const mode = await screen.findByRole("button", { name: "Switch to English" });
    expect(mode).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Pause capture" }),
    ).toBeDisabled();
    // Local chrome keeps working while the daemon is down.
    expect(
      screen.getByRole("button", { name: "Open settings" }),
    ).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "Drag to move quick bar" }),
    ).toBeEnabled();
    expect(screen.getByLabelText("Daemon unavailable")).toBeInTheDocument();
  });

  test("state load failure shows retry; action failure shows a toast only", async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === "quickbar_state") {
        return Promise.reject({ code: "daemon_unavailable", message: "down" });
      }
      return Promise.reject({ code: "daemon_unavailable", message: "down" });
    });
    renderPage();
    const retry = await screen.findByRole("button", {
      name: "Retry daemon connection",
    });
    // Retry re-reads state without crashing.
    invokeMock.mockImplementation((command: string) => {
      if (command === "quickbar_state") {
        return Promise.resolve({ ...ONLINE });
      }
      return Promise.reject({ code: "daemon_unavailable", message: "down" });
    });
    fireEvent.click(retry);
    expect(
      await screen.findByRole("button", { name: "Switch to English" }),
    ).toBeInTheDocument();
  });

  test("failed toggle surfaces the typed error as a toast", async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === "quickbar_state") {
        return Promise.resolve({ ...ONLINE });
      }
      if (command === "quickbar_toggle_mode") {
        return Promise.reject({ code: "InvalidSettings", message: "bad" });
      }
      return Promise.reject({ code: "daemon_unavailable", message: "down" });
    });
    renderPage();
    fireEvent.click(
      await screen.findByRole("button", { name: "Switch to English" }),
    );
    expect(
      await screen.findByText("One or more settings values are invalid."),
    ).toBeInTheDocument();
  });

  test("applies the persisted opacity to the bar", async () => {
    mockOnline({ ...ONLINE, opacity: 0.65 });
    const { container } = renderPage();
    await screen.findByRole("toolbar", { name: "Okkhor quick bar" });
    const bar = container.querySelector('[role="toolbar"]');
    expect(bar?.getAttribute("style")).toContain("opacity: 0.65");
  });

  test("bar exposes only the five typing-surface controls", async () => {
    mockOnline();
    const { container } = renderPage();
    await screen.findByRole("toolbar", { name: "Okkhor quick bar" });
    const names = [...container.querySelectorAll("button")].map((button) =>
      button.getAttribute("aria-label"),
    );
    expect(names).toEqual([
      "Switch to English",
      "Pause capture",
      "Add the last typed word to my dictionary",
      "Open settings",
    ]);
    // The grip is a role=button div (it is also the drag surface).
    expect(
      screen.getByRole("button", { name: "Drag to move quick bar" }),
    ).toBeInTheDocument();
  });

  test("bar is full-width 52px with no clipping and no native drag regions", async () => {
    mockOnline();
    const { container } = renderPage();
    const bar = await screen.findByRole("toolbar", { name: "Okkhor quick bar" });
    // Responsive canvas: fills the fixed 340x52 window, never scrolls.
    expect(bar.className).toContain("w-full");
    expect(bar.className).toContain("h-[52px]");
    expect(bar.className).toContain("overflow-hidden");
    expect(bar.className).toContain("flex-nowrap");
    // Dragging is manual pointercapture; no button uses the Tauri drag region.
    expect(container.querySelectorAll("[data-tauri-drag-region]")).toHaveLength(0);
    const buttons = container.querySelectorAll("button");
    expect(buttons.length).toBeGreaterThan(0);
    for (const button of buttons) {
      expect(button.hasAttribute("data-tauri-drag-region")).toBe(false);
    }
  });

  test("collapsed orb is 44x44 with its own drag ring and expand button", async () => {
    mockOnline({ ...ONLINE, collapsed: true });
    renderPage();
    const orb = await screen.findByRole("button", { name: "Expand quick bar" });
    const ring = orb.parentElement;
    expect(ring?.className).toContain("w-[44px]");
    expect(ring?.className).toContain("h-[44px]");
    expect(ring?.className).toContain("overflow-hidden");
    expect(orb.className).toContain("size-9");
  });

  test("quickbar canvas lock constrains html/body without scrollbars", async () => {
    mockOnline();
    renderPage();
    await screen.findByRole("toolbar", { name: "Okkhor quick bar" });
    for (const element of [document.documentElement, document.body]) {
      const style = element.getAttribute("style") ?? "";
      expect(style).toContain("overflow:hidden");
    }
  });
});
