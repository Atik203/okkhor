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
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "./lib/ipc";
import { applyTheme, watchSystemTheme } from "./lib/theme";
import { cn } from "./lib/utils";
import { ConnectionBanner, useConnection } from "./components/connection-banner";
import StatusPage from "./pages/status";
import AboutPage from "./pages/about";

// Route-level code splitting: the settings form (react-hook-form + zod), the
// keyboard (Unicode table + emoji list), the word manager, and the quick bar
// load on demand, keeping the initial JS bundle under the 350 KiB gzip budget.
const SettingsPage = React.lazy(() => import("./pages/settings"));
const KeyboardPage = React.lazy(() => import("./pages/keyboard"));
const WordsPage = React.lazy(() => import("./pages/words"));
const QuickbarPage = React.lazy(() => import("./pages/quickbar"));

export type Route = "/" | "/settings" | "/keyboard" | "/words" | "/about" | "/quickbar";

const ROUTES: { path: Route; label: string; icon: React.ReactNode }[] = [
  { path: "/", label: "Status", icon: <Gauge className="size-4" aria-hidden="true" /> },
  { path: "/settings", label: "Settings", icon: <SettingsIcon className="size-4" aria-hidden="true" /> },
  { path: "/keyboard", label: "Keyboard", icon: <Keyboard className="size-4" aria-hidden="true" /> },
  { path: "/words", label: "Words", icon: <BookOpenText className="size-4" aria-hidden="true" /> },
  { path: "/about", label: "About", icon: <Info className="size-4" aria-hidden="true" /> },
];

function routeFromHash(): Route {
  const hash = window.location.hash.replace(/^#/, "");
  // The bar route has no nav entry (the `quickbar` window renders it alone).
  if (hash === "/quickbar") {
    return "/quickbar";
  }
  const found = ROUTES.find((route) => route.path === hash);
  // Unknown routes fall back to status (contract §5).
  return found ? found.path : "/";
}

export function isKnownRoute(hash: string): boolean {
  const path = hash.replace(/^#/, "");
  if (path === "/quickbar") {
    return true;
  }
  return ROUTES.some((route) => route.path === path);
}

export default function App() {
  const [route, setRoute] = React.useState<Route>(() => routeFromHash());
  // The `quickbar` Tauri window hosts only the bar: detect the window label
  // (outside Tauri — e.g. `npm run dev` in a browser — fall back to the hash).
  const [quickbarWindow, setQuickbarWindow] = React.useState(false);
  React.useEffect(() => {
    let cancelled = false;
    try {
      const label = getCurrentWindow().label;
      if (!cancelled && label === "quickbar") {
        setQuickbarWindow(true);
      }
    } catch {
      // Not running inside Tauri (browser dev): hash routing decides.
    }
    return () => {
      cancelled = true;
    };
  }, []);
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

  // `quickbar_open_settings` shows this process's `main` window and emits
  // `okkhor:navigate`; honor it even when the user is on another tab.
  React.useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<string>("okkhor:navigate", (event) => {
      const path = event.payload;
      if (path === "/settings" || path === "/") {
        window.location.hash = `#${path}`;
      }
    })
      .then((stop) => {
        unlisten = stop;
      })
      .catch(() => {
        // Not running inside Tauri (browser dev): nothing to listen on.
      });
    return () => unlisten?.();
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

  // The quick bar window renders only the bar — no nav, no banner chrome.
  // Reachable both as the `quickbar` window and, for browser dev, `#/quickbar`.
  if (quickbarWindow || route === "/quickbar") {
    return (
      <React.Suspense
        fallback={
          <div aria-label="Loading quick bar" className="h-12 w-[340px] animate-pulse" />
        }
      >
        <QuickbarPage />
      </React.Suspense>
    );
  }

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
          <React.Suspense
            fallback={
              <div aria-label="Loading page" className="grid max-w-2xl animate-pulse gap-3">
                <div className="h-32 rounded-lg bg-neutral-100 dark:bg-neutral-800" />
              </div>
            }
          >
            {route === "/settings" && <SettingsPage readOnly={readOnly} />}
            {route === "/keyboard" && <KeyboardPage readOnly={readOnly} />}
            {route === "/words" && <WordsPage readOnly={readOnly} />}
          </React.Suspense>
          {route === "/about" && <AboutPage />}
        </main>
      </div>

      {/* aria-live announcements for save/error toasts (contract §5). */}
      <Toaster richColors closeButton aria-label="Notifications" />
    </div>
  );
}
