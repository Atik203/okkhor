import type { ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Pause, Play } from "lucide-react";
import { toast } from "sonner";
import { api } from "../lib/ipc";
import { describeError, isDaemonUnavailable } from "../lib/errors";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { Switch } from "../components/ui/switch";

function Row({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4 py-2">
      <dt className="text-sm text-neutral-500 dark:text-neutral-400">{label}</dt>
      <dd className="text-sm font-medium">{value}</dd>
    </div>
  );
}

function LoadingSkeleton() {
  return (
    <div aria-label="Loading status" className="grid animate-pulse gap-3">
      {[0, 1, 2].map((key) => (
        <div
          key={key}
          className="h-20 rounded-lg bg-neutral-100 dark:bg-neutral-800"
        />
      ))}
    </div>
  );
}

export default function StatusPage({ readOnly }: { readOnly: boolean }) {
  const queryClient = useQueryClient();
  const statusQuery = useQuery({
    queryKey: ["status"],
    queryFn: () => api.status(),
    retry: false,
  });

  const pauseMutation = useMutation({
    mutationFn: (paused: boolean) => api.setPaused(paused),
    onSuccess: (status) => {
      queryClient.setQueryData(["status"], status);
      toast.success(status.paused ? "Capture paused." : "Capture resumed.");
    },
    onError: (error) => toast.error(describeError(error)),
  });

  if (statusQuery.isPending) {
    return <LoadingSkeleton />;
  }

  if (statusQuery.isError) {
    const unavailable = isDaemonUnavailable(statusQuery.error);
    return (
      <Card>
        <CardHeader>
          <CardTitle>Status unavailable</CardTitle>
        </CardHeader>
        <CardContent className="grid gap-3">
          <p className="text-sm text-neutral-600 dark:text-neutral-300">
            {unavailable
              ? "The daemon is not running, so live status cannot be shown. Typing in other apps is unaffected."
              : describeError(statusQuery.error)}
          </p>
          <div>
            <Button
              variant="outline"
              onClick={() => void statusQuery.refetch()}
              aria-label="Retry loading status"
            >
              Retry
            </Button>
          </div>
        </CardContent>
      </Card>
    );
  }

  const status = statusQuery.data;

  return (
    <div className="grid max-w-2xl gap-4">
      <Card>
        <CardHeader>
          <CardTitle>Okkhor status</CardTitle>
        </CardHeader>
        <CardContent>
          <dl className="divide-y divide-neutral-100 dark:divide-neutral-800">
            <Row label="Daemon version" value={status.daemon_version} />
            <Row
              label="Capture"
              value={
                <span className="flex items-center gap-2">
                  <Badge variant={status.paused ? "secondary" : "default"}>
                    {status.paused ? "Paused" : "Running"}
                  </Badge>
                  <Switch
                    checked={!status.paused}
                    disabled={readOnly || pauseMutation.isPending}
                    onCheckedChange={(checked) =>
                      pauseMutation.mutate(!checked)
                    }
                    aria-label={status.paused ? "Resume capture" : "Pause capture"}
                  />
                </span>
              }
            />
            <Row
              label="Input mode"
              value={status.input_mode === "bangla" ? "Bangla" : "English"}
            />
            <Row label="Language / layout" value={`${status.language} / ${status.layout}`} />
            <Row
              label="Output encoding"
              value={status.output_encoding === "unicode" ? "Unicode" : "Bijoy"}
            />
            <Row label="Words stored" value={status.words_total} />
            <Row label="Autostart" value={status.autostart ? "On" : "Off"} />
          </dl>
        </CardContent>
      </Card>

      <Card>
        <CardContent className="flex items-start gap-3 pt-5 text-sm text-neutral-600 dark:text-neutral-300">
          {status.paused ? (
            <Pause className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          ) : (
            <Play className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          )}
          <p>
            This settings window is a separate process from typing. Closing it
            or losing the daemon connection never stops keystrokes in other
            apps.
          </p>
        </CardContent>
      </Card>
    </div>
  );
}
