import * as React from "react";
import { useQuery } from "@tanstack/react-query";
import { Loader2, TriangleAlert, RefreshCw } from "lucide-react";
import { api } from "../lib/ipc";
import {
  DAEMON_UNAVAILABLE,
  getErrorCode,
  getErrorMessage,
  isDaemonUnavailable,
} from "../lib/errors";
import { Button } from "./ui/button";

export interface ConnectionState {
  available: boolean;
  checking: boolean;
  /** The engine is being started (auto-start or an explicit retry). */
  starting: boolean;
  /** Re-probe status only. */
  retry: () => void;
  /** Start the engine if needed, then re-probe. */
  ensure: () => void;
  error: unknown;
}

/**
 * Single status probe shared by the banner and the read-only guard.
 *
 * `Okkhor.exe` starts the engine next to itself when the pipe is absent, so a
 * first launch online shows "Starting…" instead of a read-only dead end. The
 * attempt is idempotent: the Rust side rate-limits spawns and re-probes.
 */
export function useConnection(): ConnectionState {
  const query = useQuery({
    queryKey: ["status"],
    queryFn: () => api.status(),
    retry: false,
    refetchOnWindowFocus: false,
    staleTime: 10_000,
  });

  const autoStarted = React.useRef(false);
  const [starting, setStarting] = React.useState(false);
  const refetch = query.refetch;

  const ensure = React.useCallback(async () => {
    setStarting(true);
    try {
      await api.ensureDaemon();
    } catch {
      // The next status probe decides what the banner shows; the typed error
      // is already sanitized and carries no user text.
    } finally {
      setStarting(false);
      void refetch();
    }
  }, [refetch]);

  // One automatic attempt on the first failed probe. Never loop: a broken
  // install stays visibly broken and offers an explicit Start button.
  React.useEffect(() => {
    if (autoStarted.current || !query.isError) {
      return;
    }
    autoStarted.current = true;
    if (getErrorCode(query.error) === DAEMON_UNAVAILABLE) {
      void ensure();
    }
  }, [query.isError, query.error, ensure]);

  const engineDown =
    query.isError &&
    (isDaemonUnavailable(query.error) ||
      getErrorCode(query.error) === "engine_missing");

  return {
    available: !engineDown,
    checking: query.isPending,
    starting,
    retry: () => void query.refetch(),
    ensure: () => void ensure(),
    error: query.isError ? query.error : null,
  };
}

/**
 * Persistent "engine is not running" banner (contract §5). The UI never
 * crashes when the pipe is absent; it renders read-only and offers a start.
 */
export function ConnectionBanner({
  connection,
}: {
  connection: ConnectionState;
}) {
  if (connection.available || connection.checking) {
    return null;
  }
  if (connection.starting) {
    return (
      <div
        role="status"
        aria-live="polite"
        className="flex items-center gap-3 border-b border-teal-300 bg-teal-50 px-4 py-2.5 text-sm text-teal-900 dark:border-teal-800 dark:bg-teal-950 dark:text-teal-100"
      >
        <Loader2 className="size-4 shrink-0 animate-spin" aria-hidden="true" />
        <p className="flex-1">Starting the Okkhor engine…</p>
      </div>
    );
  }
  const missing = getErrorCode(connection.error) === "engine_missing";
  return (
    <div
      role="alert"
      aria-live="assertive"
      className="flex items-center gap-3 border-b border-amber-300 bg-amber-50 px-4 py-2.5 text-sm text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100"
    >
      <TriangleAlert className="size-4 shrink-0" aria-hidden="true" />
      <p className="flex-1">
        {missing
          ? getErrorMessage(connection.error)
          : "Okkhor engine is not running. Settings are read-only until it starts."}
      </p>
      {missing ? null : (
        <Button
          size="sm"
          variant="outline"
          onClick={connection.ensure}
          aria-label="Start the Okkhor engine"
        >
          <RefreshCw className="size-3.5" aria-hidden="true" />
          Start Okkhor
        </Button>
      )}
    </div>
  );
}
