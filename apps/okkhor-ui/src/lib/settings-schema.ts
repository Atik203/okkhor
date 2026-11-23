import { z } from "zod";
import type { Settings } from "./ipc";

/** Layout codes known to the daemon (mirrors `okkhor-ipc` KNOWN_LAYOUTS). */
export const KNOWN_LAYOUTS = [
  "avro",
  "national",
  "probhat",
  "bijoy",
  "banglish",
  "khipro",
] as const;

const FUNCTION_KEY = /^[Ff]([1-9]|1[0-9]|2[0-4])$/;
const CHORD_PART = /^(Ctrl|Control|Alt|Shift|Win|Meta|Super|Cmd|Windows)$/i;
const NAMED_KEY =
  /^(Space|Tab|Enter|Escape|Esc|Backspace|Delete|Del|Insert|Ins|Home|End|PageUp|PageDown|Up|Down|Left|Right|[A-Za-z0-9])$/;

/** Mirror of Rust `parse_hotkey` (pause hotkey grammar). */
export function isValidPauseHotkey(text: string): boolean {
  const parts = text.split("+");
  if (parts.length < 2) {
    return false;
  }
  const seen = new Set<string>();
  for (const raw of parts.slice(0, -1)) {
    const part = raw.trim();
    if (part.length === 0 || !CHORD_PART.test(part)) {
      return false;
    }
    const key = part.toLowerCase();
    if (seen.has(key)) {
      return false;
    }
    seen.add(key);
  }
  const last = parts[parts.length - 1]?.trim() ?? "";
  if (FUNCTION_KEY.test(last)) {
    return true;
  }
  if (last.length === 1 && /[A-Za-z0-9]/.test(last)) {
    return true;
  }
  return NAMED_KEY.test(last);
}

/**
 * Mirror of Rust `parse_toggle_hotkey`: a single function key, or a valid
 * chord. Printable ASCII only, 1–32 bytes, no whitespace.
 */
export function isValidToggleHotkey(text: string): boolean {
  if (
    text.length === 0 ||
    text.length > 32 ||
    !/^[\x21-\x7e]+$/.test(text) ||
    /\s/.test(text)
  ) {
    return false;
  }
  if (FUNCTION_KEY.test(text)) {
    return true;
  }
  return isValidPauseHotkey(text);
}

const fontFamily = z
  .string()
  .trim()
  .max(64, "Font family must be at most 64 characters.")
  .refine((value) => /^[\x20-\x7e]*$/.test(value), {
    message: "Font family must be ASCII printable characters.",
  })
  .transform((value) => (value === "" ? null : value))
  .nullable();

/** Client-side mirror of the Rust settings bounds (contract §2). */
export const settingsSchema = z.object({
  language: z.enum(["bn", "en"]),
  layout: z
    .string()
    .min(1, "Layout is required.")
    .refine((value) => (KNOWN_LAYOUTS as readonly string[]).includes(value), {
      message: "Unknown layout.",
    }),
  input_mode: z.enum(["bangla", "english"]),
  output_encoding: z.enum(["unicode", "bijoy"]),
  theme: z.enum(["system", "light", "dark"]),
  accent: z.enum(["teal", "indigo", "amber", "rose"]),
  font_scale: z
    .number({ invalid_type_error: "Font scale must be a number." })
    .finite("Font scale must be finite.")
    .min(0.8, "Font scale must be at least 0.8.")
    .max(1.6, "Font scale must be at most 1.6."),
  font_family: fontFamily.optional().nullable(),
  font_unicode: z.enum(["kalpurush", "siyam-rupali", "noto-sans-bengali"]),
  font_bijoy: z.enum(["sutonnymj", "kalpurush-ansi"]),
  popup_enabled: z.boolean(),
  popup_max_items: z
    .number()
    .int()
    .min(1, "At least 1 popup item.")
    .max(10, "At most 10 popup items."),
  english_suggestions: z.boolean(),
  toggle_hotkey: z
    .string()
    .min(1, "Toggle hotkey is required.")
    .max(32, "Toggle hotkey is too long.")
    .refine(isValidToggleHotkey, { message: "Invalid toggle hotkey." }),
  pause_hotkey: z
    .string()
    .min(1, "Pause hotkey is required.")
    .refine(isValidPauseHotkey, { message: "Invalid pause hotkey." }),
  show_toggle_toast: z.boolean(),
  toggle_sound: z.boolean(),
  quickbar_visible: z.boolean(),
  quickbar_collapsed: z.boolean(),
  quickbar_x: z
    .number()
    .int()
    .min(-100_000)
    .max(100_000)
    .nullable(),
  quickbar_y: z
    .number()
    .int()
    .min(-100_000)
    .max(100_000)
    .nullable(),
  quickbar_opacity: z.number().finite().min(0.5).max(1.0),
  autostart: z.boolean(),
  learning_enabled: z.boolean(),
  word_manager_page_size: z.number().int().min(10).max(200),
  ai_enabled: z.boolean(),
  speech_enabled: z.boolean(),
});

export type SettingsForm = z.infer<typeof settingsSchema>;

export function toForm(settings: Settings): SettingsForm {
  return {
    language: settings.language,
    layout: settings.layout,
    input_mode: settings.input_mode,
    output_encoding: settings.output_encoding,
    theme: settings.theme,
    accent: settings.accent,
    font_scale: settings.font_scale,
    font_family: settings.font_family,
    font_unicode: settings.font_unicode,
    font_bijoy: settings.font_bijoy,
    popup_enabled: settings.popup_enabled,
    popup_max_items: settings.popup_max_items,
    english_suggestions: settings.english_suggestions,
    toggle_hotkey: settings.toggle_hotkey,
    pause_hotkey: settings.pause_hotkey,
    show_toggle_toast: settings.show_toggle_toast,
    toggle_sound: settings.toggle_sound,
    quickbar_visible: settings.quickbar_visible,
    quickbar_collapsed: settings.quickbar_collapsed,
    quickbar_x: settings.quickbar_x,
    quickbar_y: settings.quickbar_y,
    quickbar_opacity: settings.quickbar_opacity,
    autostart: settings.autostart,
    learning_enabled: settings.learning_enabled,
    word_manager_page_size: settings.word_manager_page_size,
    ai_enabled: settings.ai_enabled,
    speech_enabled: settings.speech_enabled,
  };
}

/** Diff a validated form against the loaded settings into a minimal patch. */
export function diffPatch(
  original: SettingsForm,
  current: SettingsForm,
): Partial<SettingsForm> {
  const patch: Partial<SettingsForm> = {};
  for (const key of Object.keys(current) as (keyof SettingsForm)[]) {
    if (current[key] !== original[key]) {
      (patch as Record<string, unknown>)[key] = current[key];
    }
  }
  return patch;
}
