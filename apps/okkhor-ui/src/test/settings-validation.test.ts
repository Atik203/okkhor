import { describe, expect, test } from "vitest";
import {
  settingsSchema,
  toForm,
  diffPatch,
  isValidPauseHotkey,
  isValidToggleHotkey,
  KNOWN_LAYOUTS,
  type SettingsForm,
} from "../lib/settings-schema";
import type { Settings } from "../lib/ipc";

function baseSettings(): Settings {
  return {
    schema: 1,
    language: "bn",
    layout: "avro",
    input_mode: "bangla",
    output_encoding: "unicode",
    theme: "system",
    accent: "teal",
    font_scale: 1.0,
    font_family: null,
    font_unicode: "kalpurush",
    font_bijoy: "sutonnymj",
    popup_enabled: true,
    popup_max_items: 5,
    toggle_hotkey: "F9",
    pause_hotkey: "Ctrl+Alt+Shift+P",
    show_toggle_toast: true,
  toggle_sound: true,
    quickbar_visible: true,
    quickbar_collapsed: false,
    quickbar_x: null,
    quickbar_y: null,
    quickbar_opacity: 0.9,
    autostart: false,
    learning_enabled: true,
    word_manager_page_size: 50,
    ai_enabled: false,
    speech_enabled: false,
  };
}

describe("settings validation mirrors the Rust rules (contract §2)", () => {
  test("daemon defaults validate cleanly", () => {
    const parsed = settingsSchema.safeParse(toForm(baseSettings()));
    expect(parsed.success).toBe(true);
  });

  test("font_scale clamps range and rejects NaN", () => {
    const form = toForm(baseSettings());
    expect(settingsSchema.safeParse({ ...form, font_scale: 0.8 }).success).toBe(true);
    expect(settingsSchema.safeParse({ ...form, font_scale: 1.6 }).success).toBe(true);
    expect(settingsSchema.safeParse({ ...form, font_scale: 0.79 }).success).toBe(false);
    expect(settingsSchema.safeParse({ ...form, font_scale: 1.61 }).success).toBe(false);
    expect(settingsSchema.safeParse({ ...form, font_scale: Number.NaN }).success).toBe(false);
  });

  test("popup_max_items and page size bounds", () => {
    const form = toForm(baseSettings());
    for (const good of [1, 5, 9, 10]) {
      expect(settingsSchema.safeParse({ ...form, popup_max_items: good }).success).toBe(true);
    }
    for (const bad of [0, 11, 2.5]) {
      expect(settingsSchema.safeParse({ ...form, popup_max_items: bad }).success).toBe(false);
    }
    expect(settingsSchema.safeParse({ ...form, word_manager_page_size: 10 }).success).toBe(true);
    expect(settingsSchema.safeParse({ ...form, word_manager_page_size: 200 }).success).toBe(true);
    expect(settingsSchema.safeParse({ ...form, word_manager_page_size: 9 }).success).toBe(false);
    expect(settingsSchema.safeParse({ ...form, word_manager_page_size: 201 }).success).toBe(false);
  });

  test("unknown layout is rejected; every known layout passes", () => {
    const form = toForm(baseSettings());
    expect(settingsSchema.safeParse({ ...form, layout: "qwerty" }).success).toBe(false);
    expect(settingsSchema.safeParse({ ...form, layout: "Avro" }).success).toBe(false);
    for (const layout of KNOWN_LAYOUTS) {
      expect(settingsSchema.safeParse({ ...form, layout }).success).toBe(true);
    }
  });

  test("toggle hotkey accepts F-keys and chords, rejects bare keys", () => {
    for (const good of ["F9", "f9", "F1", "F24", "Ctrl+Space", "Ctrl+Alt+F9"]) {
      expect(isValidToggleHotkey(good)).toBe(true);
    }
    for (const bad of ["", "P", "Space", " F9", "F9 ", "F0", "F25", "F01"]) {
      expect(isValidToggleHotkey(bad)).toBe(false);
    }
    const form = toForm(baseSettings());
    expect(settingsSchema.safeParse({ ...form, toggle_hotkey: "F9" }).success).toBe(true);
    expect(settingsSchema.safeParse({ ...form, toggle_hotkey: "bogus" }).success).toBe(false);
  });

  test("pause hotkey grammar mirrors parse_hotkey", () => {
    for (const good of ["Ctrl+Alt+P", "Ctrl+Shift+F5", "Alt+Space", "Win+Enter", "Ctrl+1"]) {
      expect(isValidPauseHotkey(good)).toBe(true);
    }
    for (const bad of ["", "P", "Ctrl", "Ctrl+", "Ctrl+Ctrl+P", "Ctrl+Foo", "Ctrl+F25"]) {
      expect(isValidPauseHotkey(bad)).toBe(false);
    }
  });

  test("font family trims, caps at 64 ASCII chars", () => {
    const form = toForm(baseSettings());
    const trimmed = settingsSchema.safeParse({ ...form, font_family: "  Noto  " });
    expect(trimmed.success).toBe(true);
    if (trimmed.success) {
      expect(trimmed.data.font_family).toBe("Noto");
    }
    expect(settingsSchema.safeParse({ ...form, font_family: "x".repeat(64) }).success).toBe(true);
    expect(settingsSchema.safeParse({ ...form, font_family: "x".repeat(65) }).success).toBe(false);
    expect(settingsSchema.safeParse({ ...form, font_family: "অভ্র" }).success).toBe(false);
    expect(settingsSchema.safeParse({ ...form, font_family: "a\nb" }).success).toBe(false);
  });

  test("quickbar opacity and coordinates bounds", () => {
    const form = toForm(baseSettings());
    expect(settingsSchema.safeParse({ ...form, quickbar_opacity: 0.5 }).success).toBe(true);
    expect(settingsSchema.safeParse({ ...form, quickbar_opacity: 1.0 }).success).toBe(true);
    expect(settingsSchema.safeParse({ ...form, quickbar_opacity: 0.49 }).success).toBe(false);
    expect(settingsSchema.safeParse({ ...form, quickbar_opacity: Number.NaN }).success).toBe(false);
    expect(settingsSchema.safeParse({ ...form, quickbar_x: -100_000 }).success).toBe(true);
    expect(settingsSchema.safeParse({ ...form, quickbar_x: 100_001 }).success).toBe(false);
    expect(settingsSchema.safeParse({ ...form, quickbar_y: null }).success).toBe(true);
  });

  test("strict enums reject unknown values; font_unicode falls back on load", () => {
    const form = toForm(baseSettings());
    expect(settingsSchema.safeParse({ ...form, theme: "neon" }).success).toBe(false);
    expect(settingsSchema.safeParse({ ...form, accent: "pink" }).success).toBe(false);
    // toForm passes the daemon value through; an unknown font_unicode from a
    // newer file is normalized by the daemon, the UI only accepts known ones.
    expect(settingsSchema.safeParse({ ...form, font_unicode: "kalpurush" }).success).toBe(true);
    expect(settingsSchema.safeParse({ ...form, font_bijoy: "vrinda" }).success).toBe(false);
  });

  test("diffPatch sends only changed fields", () => {
    const original: SettingsForm = toForm(baseSettings());
    expect(diffPatch(original, { ...original })).toEqual({});
    const changed = diffPatch(original, { ...original, theme: "dark", popup_max_items: 7 });
    expect(changed).toEqual({ theme: "dark", popup_max_items: 7 });
  });
});
