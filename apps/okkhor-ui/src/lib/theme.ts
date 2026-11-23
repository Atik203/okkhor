import type { Settings } from "./ipc";

export type ThemeSetting = Settings["theme"];
export type AccentSetting = Settings["accent"];

function systemPrefersDark(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-color-scheme: dark)").matches
  );
}

export function resolveTheme(theme: ThemeSetting): "light" | "dark" {
  if (theme === "system") {
    return systemPrefersDark() ? "dark" : "light";
  }
  return theme;
}

/**
 * Apply theme + accent + font scale to the document. Called live (preview is
 * local state only) and after settings load/save.
 */
export function applyTheme(
  theme: ThemeSetting,
  accent: AccentSetting,
  fontScale: number,
): void {
  const root = document.documentElement;
  const resolved = resolveTheme(theme);
  root.classList.toggle("dark", resolved === "dark");
  root.dataset.accent = accent;
  const clamped = Number.isFinite(fontScale)
    ? Math.min(1.6, Math.max(0.8, fontScale))
    : 1;
  root.style.fontSize = `${100 * clamped}%`;
}

/** Track system theme changes while `theme === "system"`. */
export function watchSystemTheme(
  getTheme: () => ThemeSetting,
  getAccent: () => AccentSetting,
  getScale: () => number,
): () => void {
  const query = window.matchMedia("(prefers-color-scheme: dark)");
  const onChange = (): void => {
    if (getTheme() === "system") {
      applyTheme("system", getAccent(), getScale());
    }
  };
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
}
