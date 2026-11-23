import { beforeEach, describe, expect, test, vi } from "vitest";
import { applyTheme, resolveTheme } from "../lib/theme";

describe("theme application", () => {
  beforeEach(() => {
    document.documentElement.className = "";
    delete document.documentElement.dataset.accent;
    document.documentElement.style.fontSize = "";
    vi.unstubAllGlobals();
  });

  test("light and dark apply the dark class", () => {
    applyTheme("light", "teal", 1);
    expect(document.documentElement.classList.contains("dark")).toBe(false);
    applyTheme("dark", "teal", 1);
    expect(document.documentElement.classList.contains("dark")).toBe(true);
  });

  test("system follows the OS preference", () => {
    vi.stubGlobal(
      "matchMedia",
      () => ({ matches: true, addEventListener: () => undefined, removeEventListener: () => undefined }),
    );
    expect(resolveTheme("system")).toBe("dark");
    vi.stubGlobal(
      "matchMedia",
      () => ({ matches: false, addEventListener: () => undefined, removeEventListener: () => undefined }),
    );
    expect(resolveTheme("system")).toBe("light");
    expect(resolveTheme("dark")).toBe("dark");
    expect(resolveTheme("light")).toBe("light");
  });

  test("accent is stored as a data attribute", () => {
    applyTheme("light", "rose", 1);
    expect(document.documentElement.dataset.accent).toBe("rose");
    applyTheme("light", "indigo", 1);
    expect(document.documentElement.dataset.accent).toBe("indigo");
  });

  test("font scale is clamped like the Rust bounds", () => {
    applyTheme("light", "teal", 1.2);
    expect(document.documentElement.style.fontSize).toBe("120%");
    applyTheme("light", "teal", 9);
    expect(document.documentElement.style.fontSize).toBe("160%");
    applyTheme("light", "teal", 0.1);
    expect(document.documentElement.style.fontSize).toBe("80%");
    applyTheme("light", "teal", Number.NaN);
    expect(document.documentElement.style.fontSize).toBe("100%");
  });
});
