import * as React from "react";
import { listen } from "@tauri-apps/api/event";
import { Check, X } from "lucide-react";
import { api } from "../lib/ipc";
import { describeError } from "../lib/errors";
import { cn } from "../lib/utils";

/**
 * Add-word panel. Opens just below the quick bar, grows out like a tray, and
 * collapses back when saved. It is a normal focused window on purpose: the bar
 * itself is no-activate, so typing there would go to the app behind it.
 *
 * The roman input is rendered by the daemon (same engine as typing). The user
 * can pick a candidate or edit the Bangla field before saving; nothing is
 * stored until Save.
 */
export default function QuickaddPage() {
  const [roman, setRoman] = React.useState("");
  const [bangla, setBangla] = React.useState("");
  const [candidates, setCandidates] = React.useState<string[]>([]);
  const [saving, setSaving] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  const inputRef = React.useRef<HTMLInputElement>(null);
  const previewTimer = React.useRef<number | null>(null);

  const focusInput = React.useCallback(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  React.useEffect(() => {
    focusInput();
    const onWindowFocus = () => focusInput();
    window.addEventListener("focus", onWindowFocus);
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void listen("okkhor:quickadd-focus", () => focusInput()).then((stop) => {
      if (disposed) {
        stop();
      } else {
        unlisten = stop;
      }
    });
    return () => {
      disposed = true;
      unlisten?.();
      window.removeEventListener("focus", onWindowFocus);
      if (previewTimer.current != null) {
        window.clearTimeout(previewTimer.current);
      }
    };
  }, [focusInput]);

  // Debounced preview: the daemon renders the roman text exactly as typing
  // would. An empty field clears the list without an IPC round trip.
  React.useEffect(() => {
    if (previewTimer.current != null) {
      window.clearTimeout(previewTimer.current);
    }
    if (roman.trim().length === 0) {
      setCandidates([]);
      setBangla("");
      return;
    }
    previewTimer.current = window.setTimeout(() => {
      void api
        .wordsPreview(roman.trim())
        .then((words) => {
          setCandidates(words);
          setBangla((current) => (current === "" ? (words[0] ?? "") : current));
        })
        .catch(() => setCandidates([]));
    }, 140);
  }, [roman]);

  const close = React.useCallback(() => {
    void api.quickaddClose().catch(() => undefined);
  }, []);

  const save = React.useCallback(async () => {
    const trimmedRoman = roman.trim();
    const trimmedBangla = bangla.trim();
    if (trimmedRoman === "" || trimmedBangla === "") {
      setError("Enter the roman word and its Bangla form.");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await api.addWord(trimmedRoman, trimmedBangla, "bn");
      close();
    } catch (failure) {
      setError(describeError(failure));
      setSaving(false);
    }
  }, [bangla, close, roman]);

  return (
    <div
      role="dialog"
      aria-label="Add a word"
      className={cn(
        "flex h-[132px] w-full flex-col gap-1.5 overflow-hidden rounded-b-xl border border-t-0",
        "border-neutral-300 bg-white px-2 py-2 text-neutral-800 shadow-lg shadow-black/10",
        "dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100 dark:shadow-black/40",
      )}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          close();
        } else if (event.key === "Enter") {
          event.preventDefault();
          void save();
        }
      }}
    >
      <div className="flex items-center gap-1.5">
        <input
          ref={inputRef}
          value={roman}
          onChange={(event) => setRoman(event.target.value)}
          placeholder="banglish, e.g. dhonnobad"
          aria-label="Roman word"
          autoComplete="off"
          spellCheck={false}
          className="h-8 min-w-0 flex-1 rounded-md border border-neutral-300 bg-white px-2 text-[13px] outline-none focus:border-teal-500 dark:border-neutral-700 dark:bg-neutral-950"
        />
        <button
          type="button"
          aria-label="Save word"
          title="Save word"
          disabled={saving}
          onClick={() => void save()}
          className="flex size-8 shrink-0 items-center justify-center rounded-md bg-teal-700 text-white transition hover:bg-teal-600 disabled:opacity-50"
        >
          <Check aria-hidden="true" className="size-4" />
        </button>
        <button
          type="button"
          aria-label="Close"
          title="Close"
          onClick={close}
          className="flex size-8 shrink-0 items-center justify-center rounded-md border border-neutral-300 text-neutral-600 transition hover:bg-neutral-100 dark:border-neutral-700 dark:text-neutral-300 dark:hover:bg-neutral-800"
        >
          <X aria-hidden="true" className="size-4" />
        </button>
      </div>

      <input
        value={bangla}
        onChange={(event) => setBangla(event.target.value)}
        placeholder="বাংলা"
        aria-label="Bangla word"
        spellCheck={false}
        className="h-8 rounded-md border border-neutral-300 bg-white px-2 text-[13px] outline-none focus:border-teal-500 dark:border-neutral-700 dark:bg-neutral-950"
      />

      <div className="flex min-h-0 flex-1 items-center gap-1 overflow-x-auto">
        {candidates.length === 0 ? (
          <span className="truncate text-[11px] text-neutral-500">
            {error ?? "Type banglish; the Bangla form appears here."}
          </span>
        ) : (
          candidates.map((word, index) => (
            <button
              key={`${word}-${index}`}
              type="button"
              onClick={() => setBangla(word)}
              className={cn(
                "shrink-0 rounded-full border px-2 py-0.5 text-[12px] transition",
                word === bangla
                  ? "border-teal-600 bg-teal-50 text-teal-800 dark:border-teal-500 dark:bg-teal-950/60 dark:text-teal-100"
                  : "border-neutral-300 hover:bg-neutral-100 dark:border-neutral-700 dark:hover:bg-neutral-800",
              )}
            >
              {word}
            </button>
          ))
        )}
      </div>
      {error && candidates.length > 0 ? (
        <span role="alert" className="truncate text-[11px] text-red-600 dark:text-red-400">
          {error}
        </span>
      ) : null}
    </div>
  );
}
