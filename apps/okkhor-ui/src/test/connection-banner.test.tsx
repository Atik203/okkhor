import { describe, expect, test, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ConnectionBanner, type ConnectionState } from "../components/connection-banner";

function idle(): ConnectionState {
  return {
    available: true,
    checking: false,
    starting: false,
    retry: () => undefined,
    ensure: () => undefined,
    error: null,
  };
}

describe("connection banner", () => {
  test("renders nothing while connected or checking", () => {
    const { container } = render(<ConnectionBanner connection={idle()} />);
    expect(container).toBeEmptyDOMElement();
    const { container: checking } = render(
      <ConnectionBanner connection={{ ...idle(), available: false, checking: true }} />,
    );
    expect(checking).toBeEmptyDOMElement();
  });

  test("engine startup shows a quiet status line", () => {
    render(
      <ConnectionBanner
        connection={{ ...idle(), available: false, starting: true }}
      />,
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      "Starting the Okkhor engine",
    );
    expect(screen.queryByRole("button")).toBeNull();
  });

  test("a stopped engine shows an assertive banner with Start Okkhor", async () => {
    const ensure = vi.fn();
    render(
      <ConnectionBanner
        connection={{
          ...idle(),
          available: false,
          ensure,
          error: { code: "daemon_unavailable", message: "down" },
        }}
      />,
    );
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("engine is not running");
    expect(alert).toHaveTextContent("read-only");
    const button = screen.getByRole("button", { name: /start the okkhor engine/i });
    fireEvent.click(button);
    expect(ensure).toHaveBeenCalledTimes(1);
  });

  test("a missing engine explains the folder layout and hides Start", () => {
    render(
      <ConnectionBanner
        connection={{
          ...idle(),
          available: false,
          error: {
            code: "engine_missing",
            message:
              "The Okkhor engine is missing. Re-extract the portable folder and keep bin next to Okkhor.exe.",
          },
        }}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("engine is missing");
    expect(screen.queryByRole("button")).toBeNull();
  });
});
