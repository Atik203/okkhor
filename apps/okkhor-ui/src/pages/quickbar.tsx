import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import {
  PhysicalPosition,
  LogicalSize,
  getCurrentWindow,
} from "@tauri-apps/api/window";
import {
  Binary,
  Keyboard,
  Languages,
  Minimize2,
  Pause,
  Play,
  Power,
  RotateCw,
  Settings,
} from "lucide-react";
import { Toaster, toast } from "sonner";
import { api, type QuickbarSnapshot } from "../lib/ipc";
import { describeError, isDaemonUnavailable } from "../lib/errors";
import { cn } from "../lib/utils";

/**
 * Compact quick control bar (`#/quickbar`, ADR 0014 §5). Lives in the second
 * Tauri window: a horizontal pill, collapsible to a 44x44 orb, resizable
 * 320-720 px, and hideable to the tray. Never renders user text - only static
 * mode/encoding labels - and never captures keystrokes.
 *
 * Dragging is manual (pointer capture + setPosition) rather than
 * `data-tauri-drag-region`: Windows' modal move loop does not start reliably
 * for a `WS_EX_NOACTIVATE` window, but explicit positioning does and never
 * activates the bar. Every control is a real button; only the grip and the
 * empty spacer start a drag.
 */

const WIDTH_KEY = "okkhor.quickbar.width";
const MIN_WIDTH = 320;
const MAX_WIDTH = 720;
const DEFAULT_WIDTH = 460;
const COMPACT_WIDTH = 420;

function clampWidth(value: number): number {
  if (!Number.isFinite(value)) {
    return DEFAULT_WIDTH;
  }
  return Math.round(Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, value)));
}

function readStoredWidth(): number {
  try {
    const raw = window.localStorage.getItem(WIDTH_KEY);
    if (raw == null || raw === "") {
      return DEFAULT_WIDTH;
    }
    return clampWidth(Number(raw));
  } catch {
    return DEFAULT_WIDTH;
  }
}

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
    const lock =
      "width:100%;height:100%;margin:0;padding:0;overflow:hidden;background:transparent;";
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

/** Manual window drag that works on a non-activating window. */
function useBarDrag() {
  const win = React.useMemo(() => getCurrentWindow(), []);
  const drag = React.useRef<{
    x0: number;
    y0: number;
    ox: number;
    oy: number;
    scale: number;
    ready: boolean;
    moved: boolean;
    last: number;
  } | null>(null);
  const [dragging, setDragging] = React.useState(false);

  const begin = React.useCallback(
    (event: React.PointerEvent<HTMLElement>) => {
      if (event.button !== 0 || !event.isPrimary) {
        return;
      }
      event.preventDefault();
      event.currentTarget.setPointerCapture(event.pointerId);
      drag.current = {
        x0: event.screenX,
        y0: event.screenY,
        ox: 0,
        oy: 0,
        scale: window.devicePixelRatio || 1,
        ready: false,
        moved: false,
        last: 0,
      };
      setDragging(true);
      void win
        .outerPosition()
        .then((position) => {
          if (drag.current) {
            drag.current.ox = position.x;
            drag.current.oy = position.y;
            drag.current.ready = true;
          }
        })
        .catch(() => {
          drag.current = null;
          setDragging(false);
        });
    },
    [win],
  );

  const move = React.useCallback(
    (event: React.PointerEvent<HTMLElement>) => {
      const state = drag.current;
      if (!state || !state.ready) {
        return;
      }
      const now = performance.now();
      if (now - state.last < 16) {
        return;
      }
      state.last = now;
      const dx = (event.screenX - state.x0) * state.scale;
      const dy = (event.screenY - state.y0) * state.scale;
      if (!state.moved && Math.hypot(dx, dy) < 3) {
        return;
      }
      state.moved = true;
      void win
        .setPosition(
          new PhysicalPosition(
            Math.round(state.ox + dx),
            Math.round(state.oy + dy),
          ),
        )
        .catch(() => undefined);
    },
    [win],
  );

  const end = React.useCallback((event: React.PointerEvent<HTMLElement>) => {
    const state = drag.current;
    drag.current = null;
    setDragging(false);
    try {
      event.currentTarget.releasePointerCapture(event.pointerId);
    } catch {
      // Capture may already be gone after a window move; nothing to release.
    }
    if (state?.moved) {
      void api.quickbarSavePosition().catch(() => undefined);
    }
  }, []);

  const cancel = React.useCallback(() => {
    drag.current = null;
    setDragging(false);
  }, []);

  return { dragging, begin, move, end, cancel };
}

/** Width state kept in localStorage, applied with a logical resize. */
function useBarWidth(active: boolean) {
  const win = React.useMemo(() => getCurrentWindow(), []);
  const [width, setWidth] = React.useState(readStoredWidth);

  React.useEffect(() => {
    if (!active) {
      return;
    }
    try {
      window.localStorage.setItem(WIDTH_KEY, String(width));
    } catch {
      // Private/storage-disabled: width still applies for this session.
    }
    void win.setSize(new LogicalSize(width, 60)).catch(() => undefined);
  }, [active, width, win]);

  React.useEffect(() => {
    if (!active) {
      return;
    }
    const onResize = () => {
      const actual = clampWidth(window.innerWidth);
      setWidth((current) => (Math.abs(actual - current) > 1 ? actual : current));
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [active]);

  return [width, setWidth] as const;
}

/** Right-edge resize handle (320-720 logical px). */
function useHorizontalResize(
  width: number,
  setWidth: React.Dispatch<React.SetStateAction<number>>,
) {
  const state = React.useRef<{ x0: number; width0: number } | null>(null);

  const begin = React.useCallback(
    (event: React.PointerEvent<HTMLElement>) => {
      if (event.button !== 0 || !event.isPrimary) {
        return;
      }
      event.preventDefault();
      event.currentTarget.setPointerCapture(event.pointerId);
      state.current = { x0: event.screenX, width0: width };
    },
    [width],
  );
  const move = React.useCallback(
    (event: React.PointerEvent<HTMLElement>) => {
      if (!state.current) {
        return;
      }
      setWidth(clampWidth(state.current.width0 + (event.screenX - state.current.x0)));
    },
    [setWidth],
  );
  const end = React.useCallback((event: React.PointerEvent<HTMLElement>) => {
    state.current = null;
    try {
      event.currentTarget.releasePointerCapture(event.pointerId);
    } catch {
      // Already released.
    }
  }, []);

  return { begin, move, end };
}

function Chip({
  label,
  title,
  onClick,
  disabled,
  active,
  compact,
  icon,
}: {
  label: string;
  title: string;
  onClick: () => void;
  disabled?: boolean;
  active?: boolean;
  compact: boolean;
  icon: React.ReactNode;
}) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      disabled={disabled}
      onClick={onClick}
      className={cn(
        "flex h-8 shrink-0 items-center justify-center whitespace-nowrap rounded-full font-semibold",
        "border border-neutral-300 bg-white text-neutral-800",
        "transition hover:bg-neutral-100 active:scale-95 disabled:cursor-not-allowed disabled:opacity-50 disabled:active:scale-100",
        "dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100 dark:hover:bg-neutral-800",
        compact ? "w-9 px-0" : "gap-1.5 px-3 text-[13px]",
        active &&
          "border-teal-600 bg-teal-50 text-teal-800 dark:border-teal-500 dark:bg-teal-950/60 dark:text-teal-100",
      )}
    >
      <span className="[&>svg]:size-4">{icon}</span>
      {!compact && <span>{label}</span>}
    </button>
  );
}

function IconButton({
  title,
  onClick,
  disabled,
  children,
  pressed,
  danger,
}: {
  title: string;
  onClick: () => void;
  disabled?: boolean;
  children: React.ReactNode;
  pressed?: boolean;
  danger?: boolean;
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
        danger &&
          "border-red-500 bg-red-50 text-red-700 hover:bg-red-100 dark:border-red-500 dark:bg-red-950/60 dark:text-red-200 dark:hover:bg-red-900/60",
        "[&>svg]:size-4",
      )}
    >
      {children}
    </button>
  );
}

/** Dedicated drag surfaces: the grip strip and the empty spacer. */
function DragGrip({
  dragging,
  handlers,
  onCollapse,
}: {
  dragging: boolean;
  handlers: Pick<ReturnType<typeof useBarDrag>, "begin" | "move" | "end" | "cancel">;
  onCollapse: () => void;
}) {
  return (
    <div
      role="button"
      tabIndex={-1}
      aria-label="Drag to move quick bar"
      title="Drag to move; double-click to collapse"
      onPointerDown={handlers.begin}
      onPointerMove={handlers.move}
      onPointerUp={handlers.end}
      onPointerCancel={handlers.cancel}
      onDoubleClick={onCollapse}
      className={cn(
        "flex h-full w-[18px] shrink-0 touch-none items-center justify-center rounded-l-xl",
        "text-neutral-400 transition hover:bg-neutral-100 hover:text-neutral-600",
        "dark:text-neutral-500 dark:hover:bg-neutral-800 dark:hover:text-neutral-300",
        dragging ? "cursor-grabbing bg-neutral-100 dark:bg-neutral-800" : "cursor-grab",
      )}
    >
      <span className="flex flex-col gap-[3px]">
        {[0, 1, 2, 3, 4].map((dot) => (
          <span
            key={dot}
            className="size-[3px] rounded-full bg-current"
            aria-hidden="true"
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
  const barDrag = useBarDrag();
  const collapsed = snapshotQuery.data?.collapsed ?? false;
  const [width, setWidth] = useBarWidth(!collapsed);
  const resize = useHorizontalResize(width, setWidth);
  const [confirmQuit, setConfirmQuit] = React.useState(false);
  const quitTimer = React.useRef<number | null>(null);

  // Tray "Show quick bar" wakes this hidden webview; the command applies the
  // no-activate styles and re-shows the window.
  React.useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void listen("okkhor:quickbar-show", () => {
      void api.quickbarShow().catch(() => undefined);
    }).then((stop) => {
      if (disposed) {
        stop();
      } else {
        unlisten = stop;
      }
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  React.useEffect(
    () => () => {
      if (quitTimer.current != null) {
        window.clearTimeout(quitTimer.current);
      }
    },
    [],
  );

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
  const hideToTray = useMutation({
    mutationFn: () => api.quickbarHide(),
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

  function requestQuit() {
    if (confirmQuit) {
      if (quitTimer.current != null) {
        window.clearTimeout(quitTimer.current);
      }
      void api.quickbarQuit().catch(fail);
      return;
    }
    setConfirmQuit(true);
    toast("Quit Okkhor?", {
      description: "Click the power button again to exit the keyboard and tray.",
      duration: 3000,
    });
    quitTimer.current = window.setTimeout(() => setConfirmQuit(false), 3000);
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
  // Daemon down: local chrome (collapse/settings/hide/quit) stays enabled;
  // mode actions disable with an explanation. Never echo raw text.
  const available = snapshot?.available ?? false;
  const opacity = snapshot?.opacity ?? 0.9;
  const needsDaemon = !available;

  if (collapsed) {
    // 44x44 orb: the ring is the drag surface, the 36px button expands.
    return (
      <div
        className="relative flex h-[44px] w-[44px] items-center justify-center overflow-hidden"
        style={{ opacity }}
      >
        <div
          aria-hidden="true"
          className="absolute inset-0 touch-none rounded-full"
          onPointerDown={barDrag.begin}
          onPointerMove={barDrag.move}
          onPointerUp={barDrag.end}
          onPointerCancel={barDrag.cancel}
        />
        <button
          type="button"
          aria-label="Expand quick bar"
          title="Expand quick bar (or double-click)"
          onClick={() => void expand.mutate()}
          onDoubleClick={() => void expand.mutate()}
          className={cn(
            "flex size-9 items-center justify-center rounded-full text-sm font-bold text-white",
            "bg-teal-700 shadow-lg transition hover:bg-teal-600 active:scale-95",
            "dark:bg-teal-600 dark:hover:bg-teal-500",
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
  const compact = width < COMPACT_WIDTH;

  return (
    <div
      role="toolbar"
      aria-label="Okkhor quick bar"
      style={{ opacity }}
      className={cn(
        "relative flex h-[60px] w-full flex-nowrap items-center gap-1.5 overflow-hidden rounded-xl",
        "border border-neutral-300 bg-white text-neutral-800",
        "shadow-lg shadow-black/10",
        "dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100 dark:shadow-black/40",
        compact ? "gap-1 px-1" : "px-2",
      )}
    >
      <DragGrip
        dragging={barDrag.dragging}
        handlers={barDrag}
        onCollapse={() => void collapse.mutate()}
      />
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
        compact={compact}
        icon={<Languages aria-hidden="true" />}
        label={modeBangla ? "বাংলা" : "EN"}
        title={
          modeBangla
            ? "Switch to English"
            : "Switch to Bangla"
        }
        disabled={needsDaemon || toggleMode.isPending}
        onClick={() => void toggleMode.mutate()}
        active={modeBangla}
      />
      <Chip
        compact={compact}
        icon={<Binary aria-hidden="true" />}
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
          <Play aria-hidden="true" />
        ) : (
          <Pause aria-hidden="true" />
        )}
      </IconButton>
      {queryFailed && (
        <IconButton
          title="Retry daemon connection"
          onClick={() => void snapshotQuery.refetch()}
        >
          <RotateCw aria-hidden="true" />
        </IconButton>
      )}
      <div
        aria-hidden="true"
        className="min-w-0 flex-1 touch-none self-stretch"
        title="Drag to move"
        onPointerDown={barDrag.begin}
        onPointerMove={barDrag.move}
        onPointerUp={barDrag.end}
        onPointerCancel={barDrag.cancel}
      />
      <IconButton
        title="Hide quick bar to tray"
        onClick={() => void hideToTray.mutate()}
        disabled={hideToTray.isPending}
      >
        <Minimize2 aria-hidden="true" />
      </IconButton>
      <IconButton
        title="Open on-screen keyboard"
        onClick={() => void openKeyboard.mutate()}
      >
        <Keyboard aria-hidden="true" />
      </IconButton>
      <IconButton
        title="Open settings"
        onClick={() => void openSettings.mutate()}
      >
        <Settings aria-hidden="true" />
      </IconButton>
      <IconButton
        title={confirmQuit ? "Click again to quit Okkhor" : "Quit Okkhor"}
        onClick={requestQuit}
        danger={confirmQuit}
      >
        <Power aria-hidden="true" />
      </IconButton>
      <div
        role="separator"
        aria-label="Resize quick bar"
        title="Drag to resize; double-click to reset"
        onPointerDown={resize.begin}
        onPointerMove={resize.move}
        onPointerUp={resize.end}
        onPointerCancel={resize.end}
        onDoubleClick={() => setWidth(DEFAULT_WIDTH)}
        className={cn(
          "absolute inset-y-2 right-0 w-1.5 touch-none cursor-ew-resize rounded-full",
          "bg-neutral-300/0 transition hover:bg-teal-500/60 dark:bg-neutral-600/0",
        )}
      />
      <Toaster richColors closeButton aria-label="Notifications" />
    </div>
  );
}
