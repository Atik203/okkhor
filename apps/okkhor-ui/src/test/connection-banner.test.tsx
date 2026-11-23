import { describe, expect, test, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ConnectionBanner, type ConnectionState } from "../components/connection-banner";

function idle(): ConnectionState {
  return { available: true, checking: false, retry: () => undefined, error: null };
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

  test("daemon absence shows an assertive banner with Retry", async () => {
    const retry = vi.fn();
    render(
      <ConnectionBanner
        connection={{ available: false, checking: false, retry, error: { code: "daemon_unavailable" } }}
      />,
    );
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("daemon is not running");
    expect(alert).toHaveTextContent("read-only");
    const button = screen.getByRole("button", { name: /retry daemon connection/i });
    fireEvent.click(button);
    expect(retry).toHaveBeenCalledTimes(1);
  });
});
