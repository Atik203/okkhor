import { useQuery } from "@tanstack/react-query";
import { TriangleAlert, RefreshCw } from "lucide-react";
import { api } from "../lib/ipc";
import { isDaemonUnavailable } from "../lib/errors";
import { Button } from "./ui/button";

export interface ConnectionState {
  available: boolean;
  checking: boolean;
  retry: () => void;
  error: unknown;
}

/** Single status probe shared by the banner and the read-only guard. */
export function useConnection(): ConnectionState {
  const query = useQuery({
    queryKey: ["status"],
    queryFn: () => api.status(),
    retry: false,
    refetchOnWindowFocus: false,
    staleTime: 10_000,
  });
  return {
    available: !(
      query.isError && isDaemonUnavailable(query.error)
    ),
    checking: query.isPending,
    retry: () => void query.refetch(),
    error: query.isError ? query.error : null,
  };
}

/**
 * Persistent "daemon is not running" banner (contract §5). The UI never
 * crashes when the pipe is absent; it renders read-only and offers Retry.
 */
export function ConnectionBanner({
  connection,
}: {
  connection: ConnectionState;
}) {
  if (connection.available || connection.checking) {
    return null;
  }
  return (
    <div
      role="alert"
      aria-live="assertive"
      className="flex items-center gap-3 border-b border-amber-300 bg-amber-50 px-4 py-2.5 text-sm text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100"
    >
      <TriangleAlert className="size-4 shrink-0" aria-hidden="true" />
      <p className="flex-1">
        Okkhor daemon is not running. Settings are read-only until it starts.
      </p>
      <Button
        size="sm"
        variant="outline"
        onClick={connection.retry}
        aria-label="Retry daemon connection"
      >
        <RefreshCw className="size-3.5" aria-hidden="true" />
        Retry
      </Button>
    </div>
  );
}
