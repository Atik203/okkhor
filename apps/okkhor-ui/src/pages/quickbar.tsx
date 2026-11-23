import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Minimize2, Pause, Play, RotateCw, Settings, Keyboard } from "lucide-react";
import { Toaster, toast } from "sonner";
import { api, type QuickbarSnapshot } from "../lib/ipc";
import { describeError, isDaemonUnavailable } from "../lib/errors";
import { cn } from "../lib/utils";

/**
 * Compact quick control bar (`#/quickbar`, ADR 0014 §5). Lives in the second
 * Tauri window: 340×48 pill, collapsible to a 44×44 orb. Never renders user
 * text — only static mode/encoding labels — and never captures keystrokes.
 * The background is a `data-tauri-drag-region` so the bar drags from anywhere
 * except the buttons. Mode/encoding/pause need the daemon; collapse,
 * on-screen-keyboard, and settings are local and keep working while it is down.
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

function Chip({
  label,
  title,
  onClick,
  disabled,
  active,
}: {
  label: string;
  title: string;
  onClick: () => void;
  disabled?: boolean;
  active?: boolean;
}) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      disabled={disabled}
      onClick={onClick}
      className={cn(
        "h-7 shrink-0 rounded-full px-2.5 text-xs font-semibold leading-7",
        "border border-neutral-300 bg-white text-neutral-800",
        "hover:bg-neutral-100 disabled:cursor-not-allowed disabled:opacity-50",
        "dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100 dark:hover:bg-neutral-800",
        active && "border-teal-600 dark:border-teal-500",
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
        "flex size-7 shrink-0 items-center justify-center rounded-full",
        "border border-neutral-300 bg-white text-neutral-700",
        "hover:bg-neutral-100 disabled:cursor-not-allowed disabled:opacity-50",
        "dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200 dark:hover:bg-neutral-800",
      )}
    >
      {children}
    </button>
  );
}

export default function QuickbarPage() {
  const queryClient = useQueryClient();
  const snapshotQuery = useSnapshot();
  const applySnapshot = useApplySnapshot();

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
        className="flex h-12 w-[340px] items-center px-3"
      >
        <div className="h-7 w-full animate-pulse rounded-full bg-neutral-200 dark:bg-neutral-800" />
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
    return (
      <div
        className="flex size-11 items-center justify-center"
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
            "flex size-11 items-center justify-center rounded-full text-base font-bold text-white",
            "bg-teal-700 hover:bg-teal-600 dark:bg-teal-600 dark:hover:bg-teal-500",
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
      data-tauri-drag-region
      style={{ opacity }}
      className={cn(
        "flex h-12 w-[340px] items-center gap-1.5 rounded-xl px-2.5",
        "border border-neutral-300 bg-white/95 text-neutral-800",
        "dark:border-neutral-700 dark:bg-neutral-900/95 dark:text-neutral-100",
      )}
    >
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
      />
      <IconButton
        title={paused ? "Resume capture" : "Pause capture"}
        pressed={paused}
        disabled={needsDaemon || togglePause.isPending}
        onClick={() => void togglePause.mutate()}
      >
        {paused ? (
          <Play className="size-3.5" aria-hidden="true" />
        ) : (
          <Pause className="size-3.5" aria-hidden="true" />
        )}
      </IconButton>
      {queryFailed && (
        <IconButton
          title="Retry daemon connection"
          onClick={() => void snapshotQuery.refetch()}
        >
          <RotateCw className="size-3.5" aria-hidden="true" />
        </IconButton>
      )}
      <span className="min-w-0 flex-1" data-tauri-drag-region aria-hidden="true" />
      <IconButton
        title="Collapse quick bar"
        onClick={() => void collapse.mutate()}
        disabled={collapse.isPending}
      >
        <Minimize2 className="size-3.5" aria-hidden="true" />
      </IconButton>
      <IconButton title="Open on-screen keyboard" onClick={() => void openKeyboard.mutate()}>
        <Keyboard className="size-3.5" aria-hidden="true" />
      </IconButton>
      <IconButton title="Open settings" onClick={() => void openSettings.mutate()}>
        <Settings className="size-3.5" aria-hidden="true" />
      </IconButton>
      <Toaster richColors closeButton aria-label="Notifications" />
    </div>
  );
}
