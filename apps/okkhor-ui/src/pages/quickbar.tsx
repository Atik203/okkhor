import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Minimize2, Pause, Play, RotateCw, Settings, Keyboard } from "lucide-react";
import { Toaster, toast } from "sonner";
import { api, type QuickbarSnapshot } from "../lib/ipc";
import { describeError, isDaemonUnavailable } from "../lib/errors";
import { cn } from "../lib/utils";

/**
 * Compact quick control bar (`#/quickbar`, ADR 0014 §5). Lives in the second
 * Tauri window: 460×60 bar, collapsible to a 44×44 orb. Never renders user
 * text — only static mode/encoding labels — and never captures keystrokes.
 * Dragging is via the dedicated left grip strip only (`data-tauri-drag-region`
 * on the grip, never on buttons), so clicks can't start a drag and buttons
 * stay clickable under `WS_EX_NOACTIVATE`. Mode/encoding/pause need the
 * daemon; collapse, on-screen-keyboard, and settings are local and keep
 * working while it is down.
 */

function useSnapshot() {
  return useQuery({
    queryKey: ["quickbar"],
    queryFn: () => api.quickbarState(),
    retry: false,
    refetchOnWindowFocus: false,
    staleTime: 5_000,
    // Mode can change outside the bar (F9 hotkey, tray); re-sync cheaply.
    refetchInterval: 15_000,
  });
}

function useApplySnapshot() {
  const queryClient = useQueryClient();
  return (snapshot: QuickbarSnapshot) => {
    queryClient.setQueryData(["quickbar"], snapshot);
  };
}

/**
 * Lock the page canvas to the window while the bar is mounted: full-bleed,
 * no margin/padding, no scrollbars, transparent background. Restored on
 * unmount so the normal Settings UI is untouched.
 */
function useLockCanvas() {
  React.useEffect(() => {
    const html = document.documentElement;
    const body = document.body;
    const root = document.getElementById("root");
    const prev = {
      html: html.getAttribute("style") ?? "",
      body: body.getAttribute("style") ?? "",
      root: root?.getAttribute("style") ?? null,
    };
    const lock = "width:100%;height:100%;margin:0;padding:0;overflow:hidden;background:transparent;";
    html.setAttribute("style", `${prev.html};${lock}`);
    body.setAttribute("style", `${prev.body};${lock}`);
    root?.setAttribute("style", `${prev.root ?? ""};${lock}`);
    return () => {
      if (prev.html) {
        html.setAttribute("style", prev.html);
      } else {
        html.removeAttribute("style");
      }
      if (prev.body) {
        body.setAttribute("style", prev.body);
      } else {
        body.removeAttribute("style");
      }
      if (root) {
        if (prev.root) {
          root.setAttribute("style", prev.root);
        } else {
          root.removeAttribute("style");
        }
      }
    };
  }, []);
}

function Chip({
  label,
  title,
  onClick,
  disabled,
  active,
  minWidth,
}: {
  label: string;
  title: string;
  onClick: () => void;
  disabled?: boolean;
  active?: boolean;
  minWidth: string;
}) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      disabled={disabled}
      onClick={onClick}
      className={cn(
        "h-8 shrink-0 whitespace-nowrap rounded-full px-3 text-[13px] font-semibold leading-8",
        minWidth,
        "border border-neutral-300 bg-white text-neutral-800",
        "transition hover:bg-neutral-100 active:scale-95 disabled:cursor-not-allowed disabled:opacity-50 disabled:active:scale-100",
        "dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100 dark:hover:bg-neutral-800",
        active &&
          "border-teal-600 bg-teal-50 dark:border-teal-500 dark:bg-teal-950/50",
      )}
    >
      {label}
    </button>
  );
}

function IconButton({
  title,
  onClick,
  disabled,
  children,
  pressed,
}: {
  title: string;
  onClick: () => void;
  disabled?: boolean;
  children: React.ReactNode;
  pressed?: boolean;
}) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      aria-pressed={pressed}
      disabled={disabled}
      onClick={onClick}
      className={cn(
        "flex size-8 shrink-0 items-center justify-center rounded-full",
        "border border-neutral-300 bg-white text-neutral-700",
        "transition hover:bg-neutral-100 active:scale-95 disabled:cursor-not-allowed disabled:opacity-50 disabled:active:scale-100",
        "dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200 dark:hover:bg-neutral-800",
        pressed &&
          "border-teal-600 bg-teal-50 text-teal-800 dark:border-teal-500 dark:bg-teal-950/50 dark:text-teal-200",
      )}
    >
      {children}
    </button>
  );
}

/** Dedicated drag handle: the only draggable surface on the bar. */
function DragGrip() {
  return (
    <div
      data-tauri-drag-region
      aria-hidden="true"
      title="Drag to move"
      className="flex h-full w-4 shrink-0 cursor-grab items-center justify-center active:cursor-grabbing"
    >
      <span className="flex flex-col gap-[3px]">
        {[0, 1, 2, 3, 4].map((dot) => (
          <span
            key={dot}
            className="size-[3px] rounded-full bg-neutral-400 dark:bg-neutral-500"
          />
        ))}
      </span>
    </div>
  );
}

export default function QuickbarPage() {
  const queryClient = useQueryClient();
  const snapshotQuery = useSnapshot();
  const applySnapshot = useApplySnapshot();
  useLockCanvas();

  const fail = (error: unknown) => toast.error(describeError(error));

  const toggleMode = useMutation({
    mutationFn: () => api.quickbarToggleMode(),
    onSuccess: applySnapshot,
    onError: fail,
  });
  const cycleEncoding = useMutation({
    mutationFn: () => api.quickbarCycleEncoding(),
    onSuccess: applySnapshot,
    onError: fail,
  });
  const togglePause = useMutation({
    mutationFn: () => api.quickbarTogglePause(),
    onSuccess: (snapshot) => {
      applySnapshot(snapshot);
      queryClient.invalidateQueries({ queryKey: ["status"] });
      toast.success(snapshot.paused ? "Capture paused." : "Capture resumed.");
    },
    onError: fail,
  });
  const collapse = useMutation({
    mutationFn: () => api.quickbarCollapse(),
    onSuccess: applySnapshot,
    onError: fail,
  });
  const expand = useMutation({
    mutationFn: () => api.quickbarExpand(),
    onSuccess: applySnapshot,
    onError: fail,
  });
  const openSettings = useMutation({
    mutationFn: () => api.quickbarOpenSettings(),
    onError: (error) => {
      openFallback(error, "#/settings");
    },
  });
  const openKeyboard = useMutation({
    mutationFn: () => api.quickbarOpenKeyboard(),
    onError: (error) => {
      openFallback(error, "#/keyboard");
    },
  });

  function openFallback(error: unknown, hash: "#/settings" | "#/keyboard") {
    // Outside Tauri (browser dev) there is no main window to show:
    // fall back to the in-page route instead of a dead button.
    if (isDaemonUnavailable(error)) {
      fail(error);
      return;
    }
    try {
      window.location.hash = hash;
    } catch {
      fail(error);
    }
  }

  if (snapshotQuery.isPending) {
    return (
      <div
        aria-label="Loading quick bar"
        className="flex h-[60px] w-[460px] items-center overflow-hidden px-4"
      >
        <div className="h-8 w-full animate-pulse rounded-full bg-neutral-200 dark:bg-neutral-800" />
        <Toaster richColors closeButton aria-label="Notifications" />
      </div>
    );
  }

  const snapshot: QuickbarSnapshot | undefined = snapshotQuery.data;
  const queryFailed = snapshotQuery.isError;
  // Daemon down: local chrome (collapse/settings/retry) stays enabled;
  // mode actions disable with an explanation. Never echo raw text.
  const available = snapshot?.available ?? false;
  const collapsed = snapshot?.collapsed ?? false;
  const opacity = snapshot?.opacity ?? 0.9;
  const needsDaemon = !available;

  if (collapsed) {
    // 44×44 orb: the ring is the drag surface, the 36px button expands.
    return (
      <div
        className="flex h-[44px] w-[44px] items-center justify-center overflow-hidden"
        style={{ opacity }}
        data-tauri-drag-region
      >
        <button
          type="button"
          aria-label="Expand quick bar"
          title="Expand quick bar (or double-click)"
          onClick={() => void expand.mutate()}
          onDoubleClick={() => void expand.mutate()}
          className={cn(
            "flex size-9 items-center justify-center rounded-full text-base font-bold text-white",
            "bg-teal-700 transition hover:bg-teal-600 active:scale-95 dark:bg-teal-600 dark:hover:bg-teal-500",
          )}
        >
          <span aria-hidden="true">অ</span>
        </button>
        <Toaster richColors closeButton aria-label="Notifications" />
      </div>
    );
  }

  const modeBangla = snapshot?.input_mode !== "english";
  const encodingUnicode = snapshot?.output_encoding !== "bijoy";
  const paused = snapshot?.paused ?? false;

  return (
    <div
      role="toolbar"
      aria-label="Okkhor quick bar"
      style={{ opacity }}
      className={cn(
        "flex h-[60px] w-[460px] flex-nowrap items-center gap-1.5 overflow-hidden rounded-xl px-2",
        "border border-neutral-300 bg-white text-neutral-800",
        "shadow-lg shadow-black/10",
        "dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100 dark:shadow-black/40",
      )}
    >
      <DragGrip />
      <span
        role="status"
        aria-label={available ? "Daemon connected" : "Daemon unavailable"}
        title={available ? "Daemon connected" : "Daemon unavailable"}
        className={cn(
          "size-2 shrink-0 rounded-full",
          available ? "bg-emerald-500" : "bg-amber-500",
        )}
      />
      <span className="sr-only">
        {available ? "Daemon connected." : "Daemon unavailable."}
      </span>

      <Chip
        label={modeBangla ? "বাংলা" : "EN"}
        title={modeBangla ? "Switch to English" : "Switch to Bangla"}
        disabled={needsDaemon || toggleMode.isPending}
        onClick={() => void toggleMode.mutate()}
        active={modeBangla}
        minWidth="min-w-[76px]"
      />
      <Chip
        label={encodingUnicode ? "ইউনিকোড" : "বিজয়"}
        title={
          encodingUnicode
            ? "Switch output to Bijoy"
            : "Switch output to Unicode"
        }
        disabled={needsDaemon || cycleEncoding.isPending}
        onClick={() => void cycleEncoding.mutate()}
        active={!encodingUnicode}
        minWidth="min-w-[104px]"
      />
      <IconButton
        title={paused ? "Resume capture" : "Pause capture"}
        pressed={paused}
        disabled={needsDaemon || togglePause.isPending}
        onClick={() => void togglePause.mutate()}
      >
        {paused ? (
          <Play className="size-4" aria-hidden="true" />
        ) : (
          <Pause className="size-4" aria-hidden="true" />
        )}
      </IconButton>
      {queryFailed && (
        <IconButton
          title="Retry daemon connection"
          onClick={() => void snapshotQuery.refetch()}
        >
          <RotateCw className="size-4" aria-hidden="true" />
        </IconButton>
      )}
      <span className="min-w-0 flex-1" aria-hidden="true" />
      <IconButton
        title="Collapse quick bar"
        onClick={() => void collapse.mutate()}
        disabled={collapse.isPending}
      >
        <Minimize2 className="size-4" aria-hidden="true" />
      </IconButton>
      <IconButton title="Open on-screen keyboard" onClick={() => void openKeyboard.mutate()}>
        <Keyboard className="size-4" aria-hidden="true" />
      </IconButton>
      <IconButton title="Open settings" onClick={() => void openSettings.mutate()}>
        <Settings className="size-4" aria-hidden="true" />
      </IconButton>
      <Toaster richColors closeButton aria-label="Notifications" />
    </div>
  );
}
