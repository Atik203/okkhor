import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { getVersion } from "@tauri-apps/api/app";
import { api } from "../lib/ipc";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";

/** The UI version comes from `tauri.conf.json` at runtime, never a literal. */
function useUiVersion(): string {
  const [version, setVersion] = useState("…");
  useEffect(() => {
    let cancelled = false;
    getVersion()
      .then((value) => {
        if (!cancelled) {
          setVersion(value);
        }
      })
      .catch(() => {
        // Browser dev (no Tauri host): there is no bundle version to read.
        if (!cancelled) {
          setVersion("dev");
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);
  return version;
}

export default function AboutPage() {
  const statusQuery = useQuery({
    queryKey: ["status"],
    queryFn: () => api.status(),
    retry: false,
  });
  const uiVersion = useUiVersion();

  return (
    <div className="grid min-w-0 max-w-2xl gap-4">
      <Card>
        <CardHeader>
          <CardTitle>About Okkhor</CardTitle>
        </CardHeader>
        <CardContent className="grid gap-2 text-sm">
          <p>
            <span className="text-neutral-500 dark:text-neutral-400">UI version: </span>
            {uiVersion}
          </p>
          <p>
            <span className="text-neutral-500 dark:text-neutral-400">Daemon version: </span>
            {statusQuery.data ? statusQuery.data.daemon_version : "unavailable (daemon not running)"}
          </p>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Data locations</CardTitle>
        </CardHeader>
        <CardContent className="grid gap-2 text-sm">
          <p>
            <span className="text-neutral-500 dark:text-neutral-400">Settings: </span>
            %LOCALAPPDATA%\Okkhor\settings.json (daemon is the only writer)
          </p>
          <p>
            <span className="text-neutral-500 dark:text-neutral-400">Words: </span>
            %LOCALAPPDATA%\Okkhor\data\words.bin (encrypted)
          </p>
          <p>
            <span className="text-neutral-500 dark:text-neutral-400">Session token: </span>
            %LOCALAPPDATA%\Okkhor\runtime\ipc.token
          </p>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Privacy</CardTitle>
        </CardHeader>
        <CardContent className="grid gap-2 text-sm text-neutral-600 dark:text-neutral-300">
          <p>
            Typing stays on this machine. Learning is local-only and encrypted
            at rest. Nothing is sent to the network — this UI makes no remote
            requests, loads no remote fonts or assets, and collects no
            telemetry.
          </p>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Licenses</CardTitle>
        </CardHeader>
        <CardContent className="text-sm text-neutral-600 dark:text-neutral-300">
          <p>See THIRD_PARTY_LICENSES.md in the Okkhor repository.</p>
        </CardContent>
      </Card>
    </div>
  );
}
