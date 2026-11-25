import { invoke } from "@tauri-apps/api/core";
import type { ImportFormat, Word, WordsPage } from "./words";

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
  english_suggestions: boolean;
  inline_preedit: boolean;
  toggle_hotkey: string;
  pause_hotkey: string;
  show_toggle_toast: boolean;
  toggle_sound: boolean;
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

export type QuickbarSnapshot = {
  input_mode: "bangla" | "english";
  output_encoding: "unicode" | "bijoy";
  paused: boolean;
  available: boolean;
  collapsed: boolean;
  opacity: number;
  visible: boolean;
};

/** Let the daemon report AI policy without exposing the provider key. */
export interface AiStatusInfo {
  enabled: boolean;
  provider: string;
  key_present: boolean;
  breaker_state: string;
  active_jobs: number;
}

/** `ai.poll` result (ADR 0018 §5); serde-tagged by `state`. */
export type AiJobState =
  | { state: "pending" }
  | {
      state: "done";
      text: string;
      model: string;
      input_tokens: number;
      output_tokens: number;
      latency_ms: number;
    }
  | { state: "failed"; code: string }
  | { state: "cancelled" };

/**
 * Thin wrappers over the Tauri Rust commands. Errors arrive as
 * `{ code, message }`; see `lib/errors.ts`.
 */
export const api = {
  ensureDaemon(): Promise<boolean> {
    return invoke<boolean>("daemon_ensure");
  },
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
  aiStatus(): Promise<AiStatusInfo> {
    return invoke<AiStatusInfo>("ipc_ai_status");
  },
  aiComplete(prompt: string): Promise<number> {
    return invoke<number>("ipc_ai_complete", { prompt });
  },
  aiPoll(jobId: number): Promise<AiJobState> {
    return invoke<AiJobState>("ipc_ai_poll", { jobId });
  },
  aiCancel(jobId: number): Promise<void> {
    return invoke<void>("ipc_ai_cancel", { jobId });
  },
  setPaused(paused: boolean): Promise<Status> {
    return invoke<Status>("ipc_set_paused", { paused });
  },
  listWords(query: string | null, offset: number, limit: number): Promise<WordsPage> {
    return invoke<WordsPage>("ipc_list_words", { query, offset, limit });
  },
  addWord(roman: string, output: string, language: "bn" | "en"): Promise<Word> {
    return invoke<Word>("ipc_add_word", { roman, output, language });
  },
  addLastWord(): Promise<Word> {
    return invoke<Word>("ipc_add_last_word");
  },
  removeWord(id: number): Promise<boolean> {
    return invoke<boolean>("ipc_remove_word", { id });
  },
  importWords(
    format: ImportFormat,
    data: string,
  ): Promise<{ imported: number; skipped: number }> {
    return invoke<{ imported: number; skipped: number }>("ipc_import_words", {
      format,
      data,
    });
  },
  exportWords(format: ImportFormat): Promise<{ format: ImportFormat; data: string }> {
    return invoke<{ format: ImportFormat; data: string }>("ipc_export_words", {
      format,
    });
  },
  quickbarState(): Promise<QuickbarSnapshot> {
    return invoke<QuickbarSnapshot>("quickbar_state");
  },
  quickbarToggleMode(): Promise<QuickbarSnapshot> {
    return invoke<QuickbarSnapshot>("quickbar_toggle_mode");
  },
  quickbarCycleEncoding(): Promise<QuickbarSnapshot> {
    return invoke<QuickbarSnapshot>("quickbar_cycle_encoding");
  },
  quickbarTogglePause(): Promise<QuickbarSnapshot> {
    return invoke<QuickbarSnapshot>("quickbar_toggle_pause");
  },
  quickbarOpenSettings(): Promise<void> {
    return invoke<void>("quickbar_open_settings");
  },
  quickbarOpenKeyboard(): Promise<void> {
    return invoke<void>("quickbar_open_keyboard");
  },
  quickbarCollapse(): Promise<QuickbarSnapshot> {
    return invoke<QuickbarSnapshot>("quickbar_collapse");
  },
  quickbarExpand(): Promise<QuickbarSnapshot> {
    return invoke<QuickbarSnapshot>("quickbar_expand");
  },
  quickbarHide(): Promise<void> {
    return invoke<void>("quickbar_hide");
  },
  quickbarShow(): Promise<void> {
    return invoke<void>("quickbar_show");
  },
  quickbarSavePosition(): Promise<void> {
    return invoke<void>("quickbar_save_position");
  },
  quickbarBeginResize(width: number): Promise<void> {
    return invoke<void>("quickbar_begin_resize", { width });
  },
  quickbarQuit(): Promise<void> {
    return invoke<void>("quickbar_quit");
  },
};
