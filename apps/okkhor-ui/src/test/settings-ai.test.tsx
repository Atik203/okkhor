import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import SettingsPage from "../pages/settings";
import type { AiJobState, AiStatusInfo } from "../lib/ipc";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

const SETTINGS = {
  schema: 1,
  language: "bn",
  layout: "avro",
  input_mode: "bangla",
  output_encoding: "unicode",
  theme: "system",
  accent: "teal",
  font_scale: 1,
  font_family: null,
  font_unicode: "kalpurush",
  font_bijoy: "sutonnymj",
  popup_enabled: true,
  popup_max_items: 5,
  english_suggestions: true,
  inline_preedit: true,
  toggle_hotkey: "F9",
  pause_hotkey: "Ctrl+Alt+Shift+P",
  show_toggle_toast: true,
  toggle_sound: true,
  quickbar_visible: true,
  quickbar_collapsed: false,
  quickbar_x: null,
  quickbar_y: null,
  quickbar_opacity: 1,
  autostart: false,
  learning_enabled: true,
  word_manager_page_size: 50,
  ai_enabled: false,
  speech_enabled: false,
};

interface BackendOptions {
  aiEnabled: boolean;
  status?: Partial<AiStatusInfo>;
  polls?: AiJobState[];
  jobId?: number;
}

/** Wire the invoke mock to a scripted daemon; no real invoke ever runs. */
function backend(options: BackendOptions): void {
  const polls = [...(options.polls ?? [])];
  const status: AiStatusInfo = {
    enabled: options.aiEnabled,
    provider: options.aiEnabled ? "openai" : "disabled",
    key_present: options.aiEnabled,
    breaker_state: "closed",
    active_jobs: 0,
    ...options.status,
  };
  invokeMock.mockImplementation((command: string) => {
    switch (command) {
      case "ipc_get_settings":
        return Promise.resolve({ ...SETTINGS, ai_enabled: options.aiEnabled });
      case "ipc_list_layouts":
        return Promise.resolve([{ code: "avro", name: "Avro" }]);
      case "ipc_ai_status":
        return Promise.resolve(status);
      case "ipc_ai_complete":
        return Promise.resolve(options.jobId ?? 7);
      case "ipc_ai_poll":
        return Promise.resolve(polls.shift() ?? { state: "pending" });
      case "ipc_ai_cancel":
        return Promise.resolve(undefined);
      default:
        return Promise.reject({ code: "daemon_unavailable", message: "down" });
    }
  });
}

function renderPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <SettingsPage readOnly={false} />
    </QueryClientProvider>,
  );
}

/** Flush microtasks and (optionally) advance fake timers inside `act`. */
async function settle(ms = 0): Promise<void> {
  await act(async () => {
    await Promise.resolve();
    await vi.advanceTimersByTimeAsync(ms);
    await Promise.resolve();
  });
}

function countCalls(command: string): number {
  return invokeMock.mock.calls.filter(([name]) => name === command).length;
}

describe("AI settings card (ADR 0018)", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  test("renders the card, the daemon status, and the privacy note", async () => {
    backend({ aiEnabled: false });
    renderPage();
    await settle(10);

    const card = screen.getByTestId("ai-card");
    expect(screen.getAllByText("AI").length).toBeGreaterThan(0);
    expect(card).toBeInTheDocument();

    const status = screen.getByTestId("ai-status");
    expect(status).toHaveTextContent("AI off");
    expect(status).toHaveTextContent("Provider: disabled");
    expect(status).toHaveTextContent("Key: not set");

    expect(
      screen.getByText("Only the text you paste here is sent. Prompts are not saved."),
    ).toBeInTheDocument();
  });

  test("Try a rewrite is disabled while AI is off", async () => {
    backend({ aiEnabled: false });
    renderPage();
    await settle();

    fireEvent.change(screen.getByLabelText("Text to rewrite"), {
      target: { value: "please rewrite this" },
    });
    expect(screen.getByRole("button", { name: /try a rewrite/i })).toBeDisabled();
  });

  test("Try a rewrite is disabled while the source is empty", async () => {
    backend({ aiEnabled: true, status: { key_present: true } });
    renderPage();
    await settle(10);

    expect(screen.getByTestId("ai-status")).toHaveTextContent("AI on");
    expect(screen.getByTestId("ai-status")).toHaveTextContent("Key: set");
    expect(screen.getByRole("button", { name: /try a rewrite/i })).toBeDisabled();
  });

  test("clicking Try a rewrite completes and polls until the result is shown", async () => {
    backend({
      aiEnabled: true,
      status: { key_present: true, provider: "openai" },
      polls: [
        { state: "pending" },
        {
          state: "done",
          text: "Rewritten paragraph.",
          model: "gpt-6-luna",
          input_tokens: 12,
          output_tokens: 8,
          latency_ms: 640,
        },
      ],
    });
    renderPage();
    await settle();

    const source = screen.getByLabelText("Text to rewrite");
    fireEvent.change(source, { target: { value: "original paragraph" } });
    const button = screen.getByRole("button", { name: /try a rewrite/i });
    expect(button).toBeEnabled();

    fireEvent.click(button);
    expect(screen.getByText("Working…")).toBeInTheDocument();
    await settle(); // complete resolves, first poll is pending
    expect(screen.getByText("Working…")).toBeInTheDocument();

    await settle(600); // next poll returns done
    expect(screen.getByLabelText("Rewrite result")).toHaveValue("Rewritten paragraph.");

    expect(invokeMock).toHaveBeenCalledWith("ipc_ai_complete", {
      prompt: "original paragraph",
    });
    expect(invokeMock).toHaveBeenCalledWith("ipc_ai_poll", { jobId: 7 });

    // The result is never inserted anywhere: the source field keeps the
    // user's text and no insert command was issued.
    expect(screen.getByLabelText("Text to rewrite")).toHaveValue("original paragraph");
    expect(countCalls("ipc_insert_text")).toBe(0);
  });

  test("Cancel calls ai.cancel and stops polling", async () => {
    backend({
      aiEnabled: true,
      status: { key_present: true },
      polls: [{ state: "pending" }, { state: "pending" }, { state: "pending" }],
    });
    renderPage();
    await settle();

    fireEvent.change(screen.getByLabelText("Text to rewrite"), {
      target: { value: "original paragraph" },
    });
    fireEvent.click(screen.getByRole("button", { name: /try a rewrite/i }));
    await settle();
    expect(screen.getByText("Working…")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /^cancel$/i }));
    await settle();

    expect(invokeMock).toHaveBeenCalledWith("ipc_ai_cancel", { jobId: 7 });
    expect(screen.getByText("Rewrite cancelled.")).toBeInTheDocument();

    const pollsAfterCancel = countCalls("ipc_ai_poll");
    await settle(1200);
    expect(countCalls("ipc_ai_poll")).toBe(pollsAfterCancel);
  });

  test("a failed job shows static mapped text, never raw provider text", async () => {
    backend({
      aiEnabled: true,
      status: { key_present: true },
      polls: [{ state: "failed", code: "AiProviderError" }],
    });
    renderPage();
    await settle();

    fireEvent.change(screen.getByLabelText("Text to rewrite"), {
      target: { value: "original paragraph" },
    });
    fireEvent.click(screen.getByRole("button", { name: /try a rewrite/i }));
    await settle();

    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("The AI provider could not complete the request");
    expect(alert).not.toHaveTextContent("AiProviderError");
    expect(screen.queryByLabelText("Rewrite result")).toBeNull();
  });
});
