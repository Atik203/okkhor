import { invoke } from "@tauri-apps/api/core";

/** Settings document shape (contract §2). Unknown fields are ignored. */
export interface Settings {
  schema: number;
  language: "bn" | "en";
  layout: string;
  input_mode: "bangla" | "english";
  output_encoding: "unicode" | "bijoy";
  theme: "system" | "light" | "dark";
  accent: "teal" | "indigo" | "amber" | "rose";
  font_scale: number;
  font_family: string | null;
  font_unicode: "kalpurush" | "siyam-rupali" | "noto-sans-bengali";
  font_bijoy: "sutonnymj" | "kalpurush-ansi";
  popup_enabled: boolean;
  popup_max_items: number;
  toggle_hotkey: string;
  pause_hotkey: string;
  show_toggle_toast: boolean;
  quickbar_visible: boolean;
  quickbar_collapsed: boolean;
  quickbar_x: number | null;
  quickbar_y: number | null;
  quickbar_opacity: number;
  autostart: boolean;
  learning_enabled: boolean;
  word_manager_page_size: number;
  ai_enabled: boolean;
  speech_enabled: boolean;
}

export type SettingsPatch = Partial<Omit<Settings, "schema">>;

export interface Status {
  daemon_version: string;
  paused: boolean;
  language: string;
  layout: string;
  input_mode: "bangla" | "english";
  output_encoding: "unicode" | "bijoy";
  popup_visible: boolean;
  words_total: number;
  learning_enabled: boolean;
  autostart: boolean;
}

export interface LayoutInfo {
  code: string;
  name: string;
}

/**
 * Thin wrappers over the Tauri Rust commands. Errors arrive as
 * `{ code, message }`; see `lib/errors.ts`.
 */
export const api = {
  status(): Promise<Status> {
    return invoke<Status>("ipc_status");
  },
  getSettings(): Promise<Settings> {
    return invoke<Settings>("ipc_get_settings");
  },
  updateSettings(patch: SettingsPatch): Promise<Settings> {
    return invoke<Settings>("ipc_update_settings", { patch });
  },
  listLayouts(): Promise<LayoutInfo[]> {
    return invoke<LayoutInfo[]>("ipc_list_layouts");
  },
  insertText(text: string): Promise<boolean> {
    return invoke<boolean>("ipc_insert_text", { text });
  },
  setPaused(paused: boolean): Promise<Status> {
    return invoke<Status>("ipc_set_paused", { paused });
  },
};
