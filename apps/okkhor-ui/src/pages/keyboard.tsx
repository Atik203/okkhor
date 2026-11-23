import * as React from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { ArrowDown, ArrowUp, Pin, PinOff, Search } from "lucide-react";
import { toast } from "sonner";
import { api } from "../lib/ipc";
import { describeError } from "../lib/errors";
import {
  EMOJI,
  FALLBACK_LAYOUTS,
  KEY_GROUPS,
  MAX_RECENTS,
  loadFavorites,
  loadGroupOrder,
  moveGroup,
  pushRecent,
  resolveKeyText,
  saveFavorites,
  saveGroupOrder,
  searchEmoji,
  toggleFavorite,
} from "../lib/keyboard-data";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "../components/ui/card";
import { Field, Label } from "../components/ui/field";
import { Input } from "../components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { cn } from "../lib/utils";

function KeyButton({
  text,
  pinned,
  disabled,
  onInsert,
  onTogglePin,
}: {
  text: string;
  pinned: boolean;
  disabled: boolean;
  onInsert: (text: string) => void;
  onTogglePin: (text: string) => void;
}) {
  return (
    <span className="inline-flex items-stretch overflow-hidden rounded-md border border-neutral-200 dark:border-neutral-700">
      <button
        type="button"
        aria-label={`Insert ${text}`}
        disabled={disabled}
        onClick={() => onInsert(text)}
        className="min-w-9 flex-1 px-2 py-1.5 text-lg leading-6 hover:bg-neutral-100 disabled:cursor-not-allowed disabled:opacity-40 dark:hover:bg-neutral-800"
      >
        {text}
      </button>
      <button
        type="button"
        aria-label={pinned ? `Unpin ${text} from favorites` : `Pin ${text} to favorites`}
        aria-pressed={pinned}
        disabled={disabled}
        onClick={() => onTogglePin(text)}
        className="border-l border-neutral-200 px-1.5 text-neutral-400 hover:bg-neutral-100 hover:text-neutral-700 disabled:cursor-not-allowed disabled:opacity-40 dark:border-neutral-700 dark:hover:bg-neutral-800 dark:hover:text-neutral-200"
      >
        {pinned ? (
          <PinOff className="size-3.5" aria-hidden="true" />
        ) : (
          <Pin className="size-3.5" aria-hidden="true" />
        )}
      </button>
    </span>
  );
}

export default function KeyboardPage({ readOnly }: { readOnly: boolean }) {
  const [layout, setLayout] = React.useState<string | null>(null);
  const [shift, setShift] = React.useState(false);
  const [order, setOrder] = React.useState<string[]>(() => loadGroupOrder());
  const [collapsed, setCollapsed] = React.useState<Record<string, boolean>>({});
  const [favorites, setFavorites] = React.useState<string[]>(() => loadFavorites());
  // Session-only: in-memory, never written to storage.
  const [recents, setRecents] = React.useState<string[]>([]);
  const [emojiQuery, setEmojiQuery] = React.useState("");
  const [dragId, setDragId] = React.useState<string | null>(null);

  const layoutsQuery = useQuery({
    queryKey: ["layouts"],
    queryFn: () => api.listLayouts(),
    retry: false,
    refetchOnWindowFocus: false,
    staleTime: 60_000,
  });
  const settingsQuery = useQuery({
    queryKey: ["settings"],
    queryFn: () => api.getSettings(),
    retry: false,
    refetchOnWindowFocus: false,
    staleTime: 10_000,
  });

  const layouts = layoutsQuery.data ?? FALLBACK_LAYOUTS;
  const usingFallbackLayouts = layoutsQuery.data === undefined;
  const activeLayout = layout ?? settingsQuery.data?.layout ?? "avro";

  const insertMutation = useMutation({
    mutationFn: (text: string) => api.insertText(text),
    onSuccess: (_ok, text) => {
      // No toast on success (§5: every insert shows a toast only on error).
      setRecents((prev) => pushRecent(prev, text));
    },
    onError: (error) => toast.error(describeError(error)),
  });

  const insert = React.useCallback(
    (text: string) => {
      if (readOnly || insertMutation.isPending) {
        return;
      }
      insertMutation.mutate(text);
    },
    [readOnly, insertMutation],
  );

  const reorder = React.useCallback(
    (id: string, direction: -1 | 1) => {
      setOrder((prev) => {
        const next = moveGroup(prev, id, direction);
        if (next !== prev) {
          saveGroupOrder(next);
        }
        return next;
      });
    },
    [],
  );

  const togglePin = React.useCallback((text: string) => {
    setFavorites((prev) => {
      const next = toggleFavorite(prev, text);
      saveFavorites(next);
      return next;
    });
  }, []);

  const onDropGroup = React.useCallback(
    (targetId: string) => {
      if (dragId === null || dragId === targetId) {
        setDragId(null);
        return;
      }
      setOrder((prev) => {
        const without = prev.filter((id) => id !== dragId);
        const targetIndex = without.indexOf(targetId);
        if (targetIndex < 0) {
          return prev;
        }
        const next = [...without];
        next.splice(targetIndex, 0, dragId);
        saveGroupOrder(next);
        return next;
      });
      setDragId(null);
    },
    [dragId],
  );

  const groupsById = React.useMemo(() => new Map(KEY_GROUPS.map((group) => [group.id, group])), []);
  const orderedGroups = order
    .map((id) => groupsById.get(id))
    .filter((group): group is NonNullable<typeof group> => group !== undefined);
  const emojiResults = React.useMemo(() => searchEmoji(emojiQuery), [emojiQuery]);

  return (
    <div className="grid max-w-3xl gap-4">
      <Card>
        <CardHeader>
          <CardTitle>On-screen keyboard</CardTitle>
          <CardDescription>
            Click a key to insert it into the focused app. Inserts go through
            the daemon and never through this window&apos;s focus.
          </CardDescription>
        </CardHeader>
        <CardContent className="grid gap-4">
          {readOnly && (
            <p role="alert" className="rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100">
              The daemon is not running, so keys are disabled. Start Okkhor and
              retry — typing in other apps is unaffected.
            </p>
          )}
          <div className="grid gap-4 sm:grid-cols-2">
            <Field>
              <Label htmlFor="osk-layout">Layout</Label>
              <Select
                value={activeLayout}
                onValueChange={setLayout}
                disabled={readOnly && layoutsQuery.isError}
              >
                <SelectTrigger id="osk-layout">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {layouts.map((item) => (
                    <SelectItem key={item.code} value={item.code}>
                      {item.name} ({item.code})
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <p className="text-xs text-neutral-500 dark:text-neutral-400">
                {usingFallbackLayouts
                  ? "Daemon layouts are unreachable; showing the bundled list."
                  : "From the daemon's layouts.list."}{" "}
                The layout applies to hardware typing (see Settings); this
                table always inserts Unicode text.
              </p>
            </Field>
            <Field>
              <span className="text-sm font-medium leading-none">
                Shift layer
              </span>
              <div>
                <Button
                  type="button"
                  variant={shift ? "default" : "outline"}
                  aria-pressed={shift}
                  disabled={readOnly}
                  onClick={() => setShift((value) => !value)}
                >
                  {shift ? "Shift: on" : "Shift: off"}
                </Button>
              </div>
              <p className="text-xs text-neutral-500 dark:text-neutral-400">
                Shift affects digits and symbols in this table.
              </p>
            </Field>
          </div>
          <p className="text-xs text-neutral-500 dark:text-neutral-400">
            Bijoy note: keys insert Unicode text. For Bijoy bytes in official
            documents, choose Output encoding → Bijoy in Settings.
          </p>
        </CardContent>
      </Card>

      {favorites.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle>Favorites</CardTitle>
            <CardDescription>Pinned keys. Stored on this machine only.</CardDescription>
          </CardHeader>
          <CardContent className="flex flex-wrap gap-1.5">
            {favorites.map((text) => (
              <KeyButton
                key={text}
                text={text}
                pinned
                disabled={readOnly}
                onInsert={insert}
                onTogglePin={togglePin}
              />
            ))}
          </CardContent>
        </Card>
      )}

      {orderedGroups.map((group, position) => {
        const isCollapsed = collapsed[group.id] ?? false;
        return (
          <section
            key={group.id}
            aria-label={`Key group: ${group.label}`}
            data-testid={`osk-group-${group.id}`}
            onDragOver={(event) => {
              if (dragId !== null) {
                event.preventDefault();
              }
            }}
            onDrop={() => onDropGroup(group.id)}
            className={cn(
              "rounded-lg border border-neutral-200 dark:border-neutral-800",
              dragId === group.id && "opacity-50",
            )}
          >
            <div className="flex items-center gap-1 px-3 py-2">
              <h2 className="flex-1 text-sm font-medium">{group.label}</h2>
              <button
                type="button"
                aria-label={`Collapse ${group.label}`}
                aria-expanded={!isCollapsed}
                onClick={() =>
                  setCollapsed((prev) => ({ ...prev, [group.id]: !(prev[group.id] ?? false) }))
                }
                className="rounded px-2 py-1 text-xs text-neutral-500 hover:bg-neutral-100 dark:text-neutral-400 dark:hover:bg-neutral-800"
              >
                {isCollapsed ? "Expand" : "Collapse"}
              </button>
              <button
                type="button"
                aria-label={`Move ${group.label} up`}
                disabled={position === 0}
                onClick={() => reorder(group.id, -1)}
                className="rounded p-1.5 text-neutral-500 hover:bg-neutral-100 disabled:opacity-30 dark:text-neutral-400 dark:hover:bg-neutral-800"
              >
                <ArrowUp className="size-4" aria-hidden="true" />
              </button>
              <button
                type="button"
                aria-label={`Move ${group.label} down`}
                disabled={position === orderedGroups.length - 1}
                onClick={() => reorder(group.id, 1)}
                className="rounded p-1.5 text-neutral-500 hover:bg-neutral-100 disabled:opacity-30 dark:text-neutral-400 dark:hover:bg-neutral-800"
              >
                <ArrowDown className="size-4" aria-hidden="true" />
              </button>
              <button
                type="button"
                aria-label={`Drag to reorder ${group.label}`}
                title="Drag to reorder"
                draggable
                onDragStart={() => setDragId(group.id)}
                onDragEnd={() => setDragId(null)}
                className="cursor-grab rounded p-1.5 text-neutral-400 hover:bg-neutral-100 active:cursor-grabbing dark:text-neutral-500 dark:hover:bg-neutral-800"
              >
                ⠿
              </button>
            </div>
            {!isCollapsed && (
              <div className="flex flex-wrap gap-1.5 border-t border-neutral-100 px-3 py-3 dark:border-neutral-800">
                {group.keys.map((key) => {
                  const text = resolveKeyText(key, shift);
                  return (
                    <KeyButton
                      key={key.base}
                      text={text}
                      pinned={favorites.includes(text)}
                      disabled={readOnly}
                      onInsert={insert}
                      onTogglePin={togglePin}
                    />
                  );
                })}
              </div>
            )}
          </section>
        );
      })}
      <p className="text-xs text-neutral-500 dark:text-neutral-400">
        Group order is remembered on this machine; no keystrokes are stored.
      </p>

      <Card>
        <CardHeader>
          <CardTitle>Recents</CardTitle>
          <CardDescription>
            Session only — cleared when this page closes, never stored.
          </CardDescription>
        </CardHeader>
        <CardContent>
          {recents.length === 0 ? (
            <p className="text-sm text-neutral-500 dark:text-neutral-400">
              Nothing inserted yet this session (most recent {MAX_RECENTS} shown here).
            </p>
          ) : (
            <div className="flex flex-wrap gap-1.5">
              {recents.map((text) => (
                <button
                  key={text}
                  type="button"
                  aria-label={`Insert recent ${text}`}
                  disabled={readOnly}
                  onClick={() => insert(text)}
                  className="rounded-md border border-neutral-200 px-2 py-1.5 text-lg leading-6 hover:bg-neutral-100 disabled:cursor-not-allowed disabled:opacity-40 dark:border-neutral-700 dark:hover:bg-neutral-800"
                >
                  {text}
                </button>
              ))}
            </div>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Emoji</CardTitle>
          <CardDescription>
            {EMOJI.length} bundled emoji, searchable in English and Bengali. No
            network — click to insert.
          </CardDescription>
        </CardHeader>
        <CardContent className="grid gap-3">
          <Field>
            <Label htmlFor="osk-emoji-search">Search emoji</Label>
            <div className="relative">
              <Search
                className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-neutral-400"
                aria-hidden="true"
              />
              <Input
                id="osk-emoji-search"
                type="search"
                placeholder="smile হাসি love …"
                value={emojiQuery}
                onChange={(event) => setEmojiQuery(event.target.value)}
                className="pl-8"
              />
            </div>
          </Field>
          {emojiResults.length === 0 ? (
            <p role="status" className="text-sm text-neutral-500 dark:text-neutral-400">
              No emoji match “{emojiQuery}”.
            </p>
          ) : (
            <div role="status" aria-label={`${emojiResults.length} emoji shown`} className="flex flex-wrap gap-1.5">
              {emojiResults.map((entry) => (
                <button
                  key={entry.ch}
                  type="button"
                  aria-label={`${entry.ch}: ${entry.en}`}
                  title={`${entry.en} · ${entry.bn}`}
                  disabled={readOnly}
                  onClick={() => insert(entry.ch)}
                  className="rounded-md border border-neutral-200 px-2 py-1.5 text-xl leading-6 hover:bg-neutral-100 disabled:cursor-not-allowed disabled:opacity-40 dark:border-neutral-700 dark:hover:bg-neutral-800"
                >
                  {entry.ch}
                </button>
              ))}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
