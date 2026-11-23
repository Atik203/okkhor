import { beforeEach, describe, expect, test, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import QuickbarPage from "../pages/quickbar";
import type { QuickbarSnapshot } from "../lib/ipc";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
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

  test("renders mode/encoding chips, pause, keyboard, settings, collapse with labels", async () => {
    mockOnline();
    renderPage();
    expect(
      await screen.findByRole("button", { name: "Switch to English" }),
    ).toHaveTextContent("বাংলা");
    expect(
      screen.getByRole("button", { name: "Switch output to Bijoy" }),
    ).toHaveTextContent("ইউনিকোড");
    expect(
      screen.getByRole("button", { name: "Pause capture" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Open settings" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Open on-screen keyboard" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Collapse quick bar" }),
    ).toBeInTheDocument();
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

  test("encoding chip cycles Unicode/Bijoy via quickbar_cycle_encoding", async () => {
    mockOnline();
    renderPage();
    fireEvent.click(
      await screen.findByRole("button", { name: "Switch output to Bijoy" }),
    );
    expect(
      await screen.findByRole("button", { name: "Switch output to Unicode" }),
    ).toHaveTextContent("বিজয়");
    expect(invokeMock).toHaveBeenCalledWith("quickbar_cycle_encoding");
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

  test("collapse shrinks to the orb; clicking the orb expands", async () => {
    mockOnline();
    renderPage();
    fireEvent.click(
      await screen.findByRole("button", { name: "Collapse quick bar" }),
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

  test("keyboard button opens the on-screen keyboard", async () => {
    mockOnline();
    renderPage();
    fireEvent.click(
      await screen.findByRole("button", { name: "Open on-screen keyboard" }),
    );
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("quickbar_open_keyboard"),
    );
  });

  test("unavailable daemon disables mode actions but keeps settings local", async () => {
    mockOnline({ ...ONLINE, available: false });
    renderPage();
    const mode = await screen.findByRole("button", { name: "Switch to English" });
    expect(mode).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Switch output to Bijoy" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Pause capture" }),
    ).toBeDisabled();
    // Local chrome keeps working while the daemon is down.
    expect(
      screen.getByRole("button", { name: "Open settings" }),
    ).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "Open on-screen keyboard" }),
    ).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "Collapse quick bar" }),
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
});
