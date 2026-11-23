import { beforeEach, describe, expect, test, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App, { isKnownRoute } from "../app";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

function renderApp(hash: string) {
  window.location.hash = hash;
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <App />
    </QueryClientProvider>,
  );
}

describe("route guard", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockImplementation((command: string) => {
      if (command === "ipc_get_settings") {
        return Promise.reject({ code: "daemon_unavailable", message: "down" });
      }
      return Promise.reject({ code: "daemon_unavailable", message: "down" });
    });
  });

  test("known routes are recognized; unknown routes are not", () => {
    expect(isKnownRoute("#/")).toBe(true);
    expect(isKnownRoute("#/settings")).toBe(true);
    expect(isKnownRoute("#/keyboard")).toBe(true);
    expect(isKnownRoute("#/words")).toBe(true);
    expect(isKnownRoute("#/about")).toBe(true);
    expect(isKnownRoute("#/nope")).toBe(false);
    expect(isKnownRoute("#/settings/extra")).toBe(false);
  });

  test("unknown hash falls back to the status route", async () => {
    renderApp("#/nope");
    await waitFor(() => {
      expect(screen.getByText("Status unavailable")).toBeInTheDocument();
    });
    expect(
      screen.getByText(
        "Okkhor engine is not running. Settings are read-only until it starts.",
      ),
    ).toBeInTheDocument();
  });

  test("about route renders the about page", async () => {
    renderApp("#/about");
    await waitFor(() => {
      expect(screen.getByText("About Okkhor")).toBeInTheDocument();
    });
    expect(screen.getByText(/no remote requests/i)).toBeInTheDocument();
  });

  test("placeholder routes render their pages, not the status fallback", async () => {
    renderApp("#/keyboard");
    await waitFor(() => {
      expect(screen.getByText("On-screen keyboard")).toBeInTheDocument();
    });
  });

  test("words route renders the word manager", async () => {
    renderApp("#/words");
    await waitFor(() => {
      expect(screen.getByText("Word manager")).toBeInTheDocument();
    });
  });
});
