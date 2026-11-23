import * as React from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Gauge,
  Settings as SettingsIcon,
  Keyboard,
  BookOpenText,
  Info,
} from "lucide-react";
import { Toaster } from "sonner";
import { api } from "./lib/ipc";
import { applyTheme, watchSystemTheme } from "./lib/theme";
import { cn } from "./lib/utils";
import { ConnectionBanner, useConnection } from "./components/connection-banner";
import StatusPage from "./pages/status";
import SettingsPage from "./pages/settings";
import KeyboardPage from "./pages/keyboard";
import WordsPage from "./pages/words";
import AboutPage from "./pages/about";

export type Route = "/" | "/settings" | "/keyboard" | "/words" | "/about";

const ROUTES: { path: Route; label: string; icon: React.ReactNode }[] = [
  { path: "/", label: "Status", icon: <Gauge className="size-4" aria-hidden="true" /> },
  { path: "/settings", label: "Settings", icon: <SettingsIcon className="size-4" aria-hidden="true" /> },
  { path: "/keyboard", label: "Keyboard", icon: <Keyboard className="size-4" aria-hidden="true" /> },
  { path: "/words", label: "Words", icon: <BookOpenText className="size-4" aria-hidden="true" /> },
  { path: "/about", label: "About", icon: <Info className="size-4" aria-hidden="true" /> },
];

function routeFromHash(): Route {
  const hash = window.location.hash.replace(/^#/, "");
  const found = ROUTES.find((route) => route.path === hash);
  // Unknown routes fall back to status (contract §5).
  return found ? found.path : "/";
}

export function isKnownRoute(hash: string): boolean {
  const path = hash.replace(/^#/, "");
  return ROUTES.some((route) => route.path === path);
}

export default function App() {
  const [route, setRoute] = React.useState<Route>(() => routeFromHash());
  const connection = useConnection();
  const settingsQuery = useQuery({
    queryKey: ["settings"],
    queryFn: () => api.getSettings(),
    retry: false,
    refetchOnWindowFocus: false,
    staleTime: 10_000,
  });

  React.useEffect(() => {
    const onHashChange = () => setRoute(routeFromHash());
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  const settings = settingsQuery.data;
  const theme = settings?.theme ?? "system";
  const accent = settings?.accent ?? "teal";
  const fontScale = settings?.font_scale ?? 1;

  // Apply theme/accent/font-scale live, including system-theme tracking.
  React.useEffect(() => {
    applyTheme(theme, accent, fontScale);
  }, [theme, accent, fontScale]);

  const themeRef = React.useRef(theme);
  const accentRef = React.useRef(accent);
  const scaleRef = React.useRef(fontScale);
  themeRef.current = theme;
  accentRef.current = accent;
  scaleRef.current = fontScale;
  React.useEffect(() => {
    return watchSystemTheme(
      () => themeRef.current,
      () => accentRef.current,
      () => scaleRef.current,
    );
  }, []);

  const readOnly = !connection.available;

  return (
    <div className="flex h-screen overflow-hidden">
      <nav
        aria-label="Primary"
        className="flex w-52 shrink-0 flex-col gap-1 border-r border-neutral-200 bg-neutral-50 p-3 dark:border-neutral-800 dark:bg-neutral-900"
      >
        <div className="flex items-center gap-2 px-2 pb-3 pt-1">
          <span
            aria-hidden="true"
            className="flex size-7 items-center justify-center rounded-md bg-teal-700 text-sm font-bold text-white"
          >
            অ
          </span>
          <span className="text-sm font-semibold">Okkhor</span>
        </div>
        {ROUTES.map((item) => (
          <a
            key={item.path}
            href={`#${item.path}`}
            aria-current={route === item.path ? "page" : undefined}
            className={cn(
              "flex items-center gap-2 rounded-md px-3 py-2 text-sm font-medium",
              route === item.path
                ? "bg-neutral-200 text-neutral-900 dark:bg-neutral-800 dark:text-neutral-100"
                : "text-neutral-600 hover:bg-neutral-100 dark:text-neutral-400 dark:hover:bg-neutral-800/60",
            )}
          >
            {item.icon}
            {item.label}
          </a>
        ))}
        <p className="mt-auto px-2 pt-3 text-xs text-neutral-500 dark:text-neutral-400">
          UI is separate from typing.
        </p>
      </nav>

      <div className="flex min-w-0 flex-1 flex-col">
        <ConnectionBanner connection={connection} />
        <main className="min-h-0 flex-1 overflow-auto p-5">
          {route === "/" && <StatusPage readOnly={readOnly} />}
          {route === "/settings" && <SettingsPage readOnly={readOnly} />}
          {route === "/keyboard" && <KeyboardPage />}
          {route === "/words" && <WordsPage />}
          {route === "/about" && <AboutPage />}
        </main>
      </div>

      {/* aria-live announcements for save/error toasts (contract §5). */}
      <Toaster richColors closeButton aria-label="Notifications" />
    </div>
  );
}
