import * as React from "react";
import { useQuery } from "@tanstack/react-query";
import { useForm, Controller, type Control } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { Copy, Loader2, Wand2 } from "lucide-react";
import { api, type AiJobState, type Settings } from "../lib/ipc";
import { describeError } from "../lib/errors";
import {
  settingsSchema,
  toForm,
  diffPatch,
  KNOWN_LAYOUTS,
  type SettingsForm,
} from "../lib/settings-schema";
import { applyTheme } from "../lib/theme";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "../components/ui/card";
import { Field, FieldError, Label } from "../components/ui/field";
import { Input } from "../components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { Switch } from "../components/ui/switch";
import { Textarea } from "../components/ui/textarea";

function ToggleRow({
  label,
  description,
  checked,
  onChange,
  disabled,
  name,
}: {
  label: string;
  description?: string;
  checked: boolean;
  onChange: (value: boolean) => void;
  disabled?: boolean;
  name: string;
}) {
  return (
    <div className="flex items-center justify-between gap-4 py-1.5">
      <div>
        <Label htmlFor={name}>{label}</Label>
        {description && (
          <p className="text-xs text-neutral-500 dark:text-neutral-400">
            {description}
          </p>
        )}
      </div>
      <Switch id={name} checked={checked} onCheckedChange={onChange} disabled={disabled} />
    </div>
  );
}

/**
 * Quick bar position: an explicit "Automatic (top-right)" toggle that clears
 * X/Y to null, plus numeric X/Y inputs when manual (issue #72).
 */
function QuickbarPosition({
  readOnly,
  x,
  y,
  xError,
  yError,
  onAuto,
  onChange,
}: {
  readOnly: boolean;
  x: number | null | undefined;
  y: number | null | undefined;
  xError?: string;
  yError?: string;
  onAuto: (automatic: boolean) => void;
  onChange: (name: "quickbar_x" | "quickbar_y", value: number | null) => void;
}) {
  const automatic = x == null && y == null;
  const numberField = (
    name: "quickbar_x" | "quickbar_y",
    value: number | null | undefined,
    error: string | undefined,
    label: string,
  ) => (
    <Field>
      <Label htmlFor={name}>{label}</Label>
      <Input
        id={name}
        type="number"
        step="1"
        min={-100_000}
        max={100_000}
        disabled={readOnly || automatic}
        value={value ?? ""}
        onChange={(event) =>
          onChange(name, event.target.value === "" ? null : Number(event.target.value))
        }
      />
      <FieldError message={error} />
    </Field>
  );
  return (
    <div className="grid gap-2 py-1.5">
      <ToggleRow
        name="quickbar_auto"
        label="Automatic position (top-right)"
        description="On keeps X/Y empty; off lets you set them explicitly."
        checked={automatic}
        onChange={onAuto}
        disabled={readOnly}
      />
      {!automatic && (
        <div className="grid gap-4 sm:grid-cols-2">
          {numberField("quickbar_x", x, xError, "Quick bar X (px)")}
          {numberField("quickbar_y", y, yError, "Quick bar Y (px)")}
        </div>
      )}
    </div>
  );
}

const AI_SOURCE_LIMIT = 20_000;
const AI_POLL_MS = 600;

type AiPhase = "idle" | "working" | "done" | "failed" | "cancelled";

/**
 * Explicit AI rewrite panel (ADR 0018, docs/ai.md).
 *
 * The right to run is decided by the daemon (`ai.status`), not by the unsaved
 * form toggle. The prompt is memory-only, the result is shown for review and
 * never inserted anywhere, and every terminal path clears its timer.
 */
function AiRewriteCard({
  control,
  readOnly,
  enabledOnSave,
}: {
  control: Control<SettingsForm>;
  readOnly: boolean;
  enabledOnSave: boolean;
}) {
  const statusQuery = useQuery({
    queryKey: ["ai-status"],
    queryFn: () => api.aiStatus(),
    retry: false,
    enabled: !readOnly,
  });
  const status = statusQuery.data;
  const aiOn = status?.enabled ?? false;
  const keyPresent = status?.key_present ?? false;

  const [source, setSource] = React.useState("");
  const [phase, setPhase] = React.useState<AiPhase>("idle");
  const [result, setResult] = React.useState<{ text: string; model: string } | null>(null);
  const [failure, setFailure] = React.useState<string | null>(null);
  const jobRef = React.useRef<number | null>(null);
  const timerRef = React.useRef<number | null>(null);
  const mountedRef = React.useRef(true);

  function stopTimer() {
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }

  // A pending job is cancelled on unmount; a late result is discarded and no
  // timer can fire against a dead component.
  React.useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      stopTimer();
      const jobId = jobRef.current;
      jobRef.current = null;
      if (jobId !== null) {
        void api.aiCancel(jobId).catch(() => undefined);
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function finish(next: AiPhase) {
    stopTimer();
    jobRef.current = null;
    setPhase(next);
  }

  async function poll(jobId: number) {
    timerRef.current = null;
    let state: AiJobState;
    try {
      state = await api.aiPoll(jobId);
    } catch (error) {
      if (!mountedRef.current || jobRef.current !== jobId) return;
      finish("failed");
      setFailure(describeError(error));
      return;
    }
    if (!mountedRef.current || jobRef.current !== jobId) return;
    if (state.state === "pending") {
      timerRef.current = window.setTimeout(() => void poll(jobId), AI_POLL_MS);
      return;
    }
    if (state.state === "done") {
      finish("done");
      setResult({ text: state.text, model: state.model });
      return;
    }
    if (state.state === "failed") {
      finish("failed");
      // Provider codes are mapped to static UI text; raw provider text is
      // never rendered.
      setFailure(describeError({ code: state.code, message: "" }));
      return;
    }
    finish("cancelled");
  }

  function stopJob(markCancelled: boolean) {
    stopTimer();
    const jobId = jobRef.current;
    jobRef.current = null;
    if (jobId !== null) {
      void api.aiCancel(jobId).catch(() => undefined);
    }
    if (markCancelled) {
      setPhase("cancelled");
      setFailure(null);
    }
  }

  async function startRewrite() {
    if (readOnly || !aiOn || !keyPresent || source.trim().length === 0) {
      return;
    }
    // Replacing a request cancels the previous one first.
    stopJob(false);
    setPhase("working");
    setResult(null);
    setFailure(null);
    let jobId: number;
    try {
      jobId = await api.aiComplete(source);
    } catch (error) {
      if (!mountedRef.current) return;
      setPhase("failed");
      setFailure(describeError(error));
      return;
    }
    if (!mountedRef.current) {
      void api.aiCancel(jobId).catch(() => undefined);
      return;
    }
    jobRef.current = jobId;
    void poll(jobId);
  }

  async function copyResult() {
    if (!result) return;
    try {
      await navigator.clipboard.writeText(result.text);
      toast.success("Copied.");
    } catch {
      toast.error("Could not copy. Select the text and copy it manually.");
    }
  }

  const canRun =
    !readOnly && aiOn && keyPresent && source.trim().length > 0 && phase !== "working";
  const togglePending = status !== undefined && enabledOnSave !== status.enabled;

  return (
    <Card data-testid="ai-card">
      <CardHeader>
        <CardTitle>AI</CardTitle>
        <CardDescription>
          Optional and off by default. Text is sent only when you click Try a rewrite.
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4">
        <Controller
          name="ai_enabled"
          control={control}
          render={({ field }) => (
            <ToggleRow
              name="ai_enabled"
              label="Enable AI text actions"
              description="Opt-in. The provider key stays in the environment; this panel only reports set or unset."
              checked={field.value}
              onChange={field.onChange}
              disabled={readOnly}
            />
          )}
        />

        <div
          data-testid="ai-status"
          className="flex flex-wrap items-center gap-2 text-xs text-neutral-600 dark:text-neutral-300"
        >
          {readOnly ? (
            <span>AI is unavailable while the engine is not running.</span>
          ) : statusQuery.isPending ? (
            <span>Checking AI status…</span>
          ) : statusQuery.isError || !status ? (
            <span>{describeError(statusQuery.error)}</span>
          ) : (
            <>
              <Badge variant={aiOn ? "default" : "secondary"}>
                {aiOn ? "AI on" : "AI off"}
              </Badge>
              <Badge variant="outline">Provider: {status.provider}</Badge>
              <Badge variant="outline">Key: {keyPresent ? "set" : "not set"}</Badge>
            </>
          )}
        </div>

        {togglePending && !readOnly && (
          <p className="text-xs text-neutral-500 dark:text-neutral-400">
            Save to apply the AI toggle.
          </p>
        )}

        <p className="text-xs text-neutral-500 dark:text-neutral-400">
          Only the text you paste here is sent. Prompts are not saved.
        </p>

        <Field>
          <Label htmlFor="ai_source">Text to rewrite</Label>
          <Textarea
            id="ai_source"
            aria-label="Text to rewrite"
            value={source}
            maxLength={AI_SOURCE_LIMIT}
            disabled={readOnly}
            placeholder="Paste a paragraph to rewrite."
            onChange={(event) => setSource(event.target.value)}
          />
          <p className="text-xs tabular-nums text-neutral-500 dark:text-neutral-400">
            {source.length.toLocaleString()} / 20,000 characters
          </p>
        </Field>

        <div className="flex flex-wrap items-center gap-2">
          <Button type="button" onClick={() => void startRewrite()} disabled={!canRun}>
            <Wand2 className="size-4" aria-hidden="true" />
            Try a rewrite
          </Button>
          {phase === "working" && (
            <>
              <span
                role="status"
                className="flex items-center gap-1.5 text-xs text-neutral-600 dark:text-neutral-300"
              >
                <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />
                Working…
              </span>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => stopJob(true)}
              >
                Cancel
              </Button>
            </>
          )}
        </div>

        {phase === "failed" && failure && (
          <p role="alert" className="text-sm text-red-600 dark:text-red-400">
            {failure}
          </p>
        )}

        {phase === "cancelled" && (
          <p role="status" className="text-xs text-neutral-500 dark:text-neutral-400">
            Rewrite cancelled.
          </p>
        )}

        {phase === "done" && result && (
          <div className="grid gap-2">
            <Label htmlFor="ai_result">Rewrite result</Label>
            <Textarea
              id="ai_result"
              aria-label="Rewrite result"
              readOnly
              value={result.text}
              className="min-h-28"
            />
            <div className="flex flex-wrap items-center gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => void copyResult()}
              >
                <Copy className="size-3.5" aria-hidden="true" />
                Copy
              </Button>
              <p className="text-xs text-neutral-500 dark:text-neutral-400">
                Review it here; nothing is inserted automatically.
              </p>
            </div>
          </div>
        )}
      </CardContent>
    </Card>
  );
}

export default function SettingsPage({ readOnly }: { readOnly: boolean }) {
  const queryClient = useQueryClient();
  const settingsQuery = useQuery({
    queryKey: ["settings"],
    queryFn: () => api.getSettings(),
    retry: false,
  });
  const layoutsQuery = useQuery({
    queryKey: ["layouts"],
    queryFn: () => api.listLayouts(),
    retry: false,
  });

  const form = useForm<SettingsForm>({
    resolver: zodResolver(settingsSchema),
    defaultValues: undefined,
  });
  const { control, register, handleSubmit, reset, watch, setValue, formState } = form;
  const { isDirty, isSubmitting, errors } = formState;

  // Populate once loaded; keep user edits afterwards.
  const loadedJson = settingsQuery.data ? JSON.stringify(settingsQuery.data) : null;
  React.useEffect(() => {
    if (settingsQuery.data) {
      reset(toForm(settingsQuery.data));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loadedJson]);

  // Dirty-state guard: warn before discarding unsaved edits.
  React.useEffect(() => {
    if (!isDirty) {
      return;
    }
    const onBeforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
    };
    window.addEventListener("beforeunload", onBeforeUnload);
    return () => window.removeEventListener("beforeunload", onBeforeUnload);
  }, [isDirty]);

  // Live preview of theme/accent/font-scale (local state only, pre-save).
  const theme = watch("theme");
  const accent = watch("accent");
  const fontScale = watch("font_scale");
  React.useEffect(() => {
    if (theme && accent && Number.isFinite(fontScale)) {
      applyTheme(theme, accent, fontScale);
    }
  }, [theme, accent, fontScale]);

  const saveMutation = useMutation({
    mutationFn: (patch: Partial<Settings>) => api.updateSettings(patch),
    onSuccess: (settings) => {
      queryClient.setQueryData(["settings"], settings);
      reset(toForm(settings));
      // The AI card gates on daemon truth, so an enable/disable toggle only
      // takes effect after this refresh.
      void queryClient.invalidateQueries({ queryKey: ["ai-status"] });
      toast.success("Settings saved.");
    },
    onError: (error) => toast.error(describeError(error)),
  });

  if (settingsQuery.isPending) {
    return (
      <div aria-label="Loading settings" className="grid max-w-2xl animate-pulse gap-3">
        {[0, 1, 2].map((key) => (
          <div key={key} className="h-32 rounded-lg bg-neutral-100 dark:bg-neutral-800" />
        ))}
      </div>
    );
  }

  if (settingsQuery.isError || !settingsQuery.data) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>Settings unavailable</CardTitle>
          <CardDescription>
            {describeError(settingsQuery.error)}
          </CardDescription>
        </CardHeader>
        <CardContent>
          <Button variant="outline" onClick={() => void settingsQuery.refetch()}>
            Retry
          </Button>
        </CardContent>
      </Card>
    );
  }

  const layouts = layoutsQuery.data?.map((item) => item.code) ?? [...KNOWN_LAYOUTS];

  const onSubmit = (values: SettingsForm) => {
    const original = toForm(settingsQuery.data);
    const patch = diffPatch(original, values);
    if (Object.keys(patch).length === 0) {
      toast.info("No changes to save.");
      return;
    }
    saveMutation.mutate(patch);
  };

  return (
    <form
      data-testid="page-settings"
      aria-label="Settings"
      className="grid min-w-0 items-start gap-4 lg:grid-cols-2 [&>*]:min-w-0"
      onSubmit={(event) => void handleSubmit(onSubmit)(event)}
    >
      <Card>
        <CardHeader>
          <CardTitle>Language and output</CardTitle>
        </CardHeader>
        <CardContent className="grid gap-4 sm:grid-cols-2">
          <Field>
            <Label htmlFor="language">Language</Label>
            <Controller
              name="language"
              control={control}
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange} disabled={readOnly}>
                  <SelectTrigger id="language">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="bn">Bangla (bn)</SelectItem>
                    <SelectItem value="en">English (en)</SelectItem>
                  </SelectContent>
                </Select>
              )}
            />
            <FieldError message={errors.language?.message} />
          </Field>
          <Field>
            <Label htmlFor="layout">Layout</Label>
            <Controller
              name="layout"
              control={control}
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange} disabled={readOnly}>
                  <SelectTrigger id="layout">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {layouts.map((code) => (
                      <SelectItem key={code} value={code}>
                        {code}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
            />
            <FieldError message={errors.layout?.message} />
          </Field>
          <Field>
            <Label htmlFor="input_mode">Input mode</Label>
            <Controller
              name="input_mode"
              control={control}
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange} disabled={readOnly}>
                  <SelectTrigger id="input_mode">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="bangla">Bangla</SelectItem>
                    <SelectItem value="english">English</SelectItem>
                  </SelectContent>
                </Select>
              )}
            />
            <FieldError message={errors.input_mode?.message} />
          </Field>
          <Field>
            <Label htmlFor="output_encoding">Output encoding</Label>
            <Controller
              name="output_encoding"
              control={control}
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange} disabled={readOnly}>
                  <SelectTrigger id="output_encoding">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="unicode">Unicode</SelectItem>
                    <SelectItem value="bijoy">Bijoy</SelectItem>
                  </SelectContent>
                </Select>
              )}
            />
            <FieldError message={errors.output_encoding?.message} />
            <p className="text-xs text-neutral-500 dark:text-neutral-400">
              <strong className="font-medium">Unicode</strong> is right for browsers, chat,
              and modern editors. <strong className="font-medium">Bijoy</strong> writes
              ANSI bytes for documents that use the SutonnyMJ font (legacy forms); without
              that font the app shows Latin letters. Pick Bijoy only when the field itself
              is set to SutonnyMJ.
            </p>
          </Field>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Appearance</CardTitle>
          <CardDescription>Previewed live; saved only when you press Save.</CardDescription>
        </CardHeader>
        <CardContent className="grid gap-4 sm:grid-cols-2">
          <Field>
            <Label htmlFor="theme">Theme</Label>
            <Controller
              name="theme"
              control={control}
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange} disabled={readOnly}>
                  <SelectTrigger id="theme">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="system">System</SelectItem>
                    <SelectItem value="light">Light</SelectItem>
                    <SelectItem value="dark">Dark</SelectItem>
                  </SelectContent>
                </Select>
              )}
            />
            <FieldError message={errors.theme?.message} />
          </Field>
          <Field>
            <Label htmlFor="accent">Accent</Label>
            <Controller
              name="accent"
              control={control}
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange} disabled={readOnly}>
                  <SelectTrigger id="accent">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="teal">Teal</SelectItem>
                    <SelectItem value="indigo">Indigo</SelectItem>
                    <SelectItem value="amber">Amber</SelectItem>
                    <SelectItem value="rose">Rose</SelectItem>
                  </SelectContent>
                </Select>
              )}
            />
            <FieldError message={errors.accent?.message} />
          </Field>
          <Field>
            <Label htmlFor="font_scale">Font scale (0.8 – 1.6)</Label>
            <Input
              id="font_scale"
              type="number"
              step="0.1"
              min={0.8}
              max={1.6}
              disabled={readOnly}
              {...register("font_scale", { valueAsNumber: true })}
            />
            <FieldError message={errors.font_scale?.message} />
          </Field>
          <Field>
            <Label htmlFor="word_manager_page_size">Word list page size (10 – 200)</Label>
            <Input
              id="word_manager_page_size"
              type="number"
              step="1"
              min={10}
              max={200}
              disabled={readOnly}
              {...register("word_manager_page_size", { valueAsNumber: true })}
            />
            <FieldError message={errors.word_manager_page_size?.message} />
          </Field>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Suggestion popup</CardTitle>
        </CardHeader>
        <CardContent className="grid gap-4 sm:grid-cols-2">
          <Field className="sm:col-span-2">
            <Controller
              name="popup_enabled"
              control={control}
              render={({ field }) => (
                <ToggleRow
                  name="popup_enabled"
                  label="Show suggestion popup"
                  checked={field.value}
                  onChange={field.onChange}
                  disabled={readOnly}
                />
              )}
            />
          </Field>
          <Field>
            <Label htmlFor="popup_max_items">Suggestions shown (1 - 10)</Label>
            <Input
              id="popup_max_items"
              type="number"
              step="1"
              min={1}
              max={10}
              disabled={readOnly}
              {...register("popup_max_items", { valueAsNumber: true })}
            />
            <p className="text-xs text-neutral-500 dark:text-neutral-400">
              8 is the default; 10 shows every digit-addressable row.
            </p>
            <FieldError message={errors.popup_max_items?.message} />
          </Field>
          <Field className="sm:col-span-2">
            <Controller
              name="english_suggestions"
              control={control}
              render={({ field }) => (
                <ToggleRow
                  name="english_suggestions"
                  label="English suggestions"
                  description="Show prefix completions in English mode. Tab accepts the highlighted word; typing is never replaced."
                  checked={field.value}
                  onChange={field.onChange}
                  disabled={readOnly}
                />
              )}
            />
          </Field>
          <Field>
            <Controller
              name="show_toggle_toast"
              control={control}
              render={({ field }) => (
                <ToggleRow
                  name="show_toggle_toast"
                  label="Show toggle toast"
                  checked={field.value}
                  onChange={field.onChange}
                  disabled={readOnly}
                />
              )}
            />
          </Field>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Hotkeys</CardTitle>
          <CardDescription>
            Toggle defaults to F9; pause to Ctrl+Alt+Shift+P.
          </CardDescription>
        </CardHeader>
        <CardContent className="grid gap-4 sm:grid-cols-2">
          <Field>
            <Label htmlFor="toggle_hotkey">Toggle Bangla/English</Label>
            <Input id="toggle_hotkey" disabled={readOnly} {...register("toggle_hotkey")} />
            <FieldError message={errors.toggle_hotkey?.message} />
          </Field>
          <Field>
            <Label htmlFor="pause_hotkey">Pause capture</Label>
            <Input id="pause_hotkey" disabled={readOnly} {...register("pause_hotkey")} />
            <FieldError message={errors.pause_hotkey?.message} />
          </Field>
          <Field className="sm:col-span-2">
            <Controller
              name="toggle_sound"
              control={control}
              render={({ field }) => (
                <ToggleRow
                  name="toggle_sound"
                  label="Play click on toggle"
                  description="Short sound when Bangla/English changes from the hotkey, tray, or bar."
                  checked={field.value}
                  onChange={field.onChange}
                  disabled={readOnly}
                />
              )}
            />
          </Field>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Behavior</CardTitle>
        </CardHeader>
        <CardContent className="divide-y divide-neutral-100 dark:divide-neutral-800">
          <Controller
            name="autostart"
            control={control}
            render={({ field }) => (
              <ToggleRow
                name="autostart"
                label="Start Okkhor with Windows"
                checked={field.value}
                onChange={field.onChange}
                disabled={readOnly}
              />
            )}
          />
          <Controller
            name="learning_enabled"
            control={control}
            render={({ field }) => (
              <ToggleRow
                name="learning_enabled"
                label="Learning"
                description="Local-only ranking; no text leaves this machine."
                checked={field.value}
                onChange={field.onChange}
                disabled={readOnly}
              />
            )}
          />
          <Controller
            name="quickbar_visible"
            control={control}
            render={({ field }) => (
              <ToggleRow
                name="quickbar_visible"
                label="Show quick bar"
                description="Compact always-on-top bar (launched with --quickbar)."
                checked={field.value}
                onChange={field.onChange}
                disabled={readOnly}
              />
            )}
          />
          <Controller
            name="quickbar_collapsed"
            control={control}
            render={({ field }) => (
              <ToggleRow
                name="quickbar_collapsed"
                label="Quick bar collapsed"
                checked={field.value}
                onChange={field.onChange}
                disabled={readOnly}
              />
            )}
          />
          <QuickbarPosition
            readOnly={readOnly}
            x={watch("quickbar_x")}
            y={watch("quickbar_y")}
            xError={errors.quickbar_x?.message}
            yError={errors.quickbar_y?.message}
            onAuto={(automatic) => {
              if (automatic) {
                setValue("quickbar_x", null, { shouldDirty: true });
                setValue("quickbar_y", null, { shouldDirty: true });
              } else {
                setValue("quickbar_x", 0, { shouldDirty: true });
                setValue("quickbar_y", 0, { shouldDirty: true });
              }
            }}
            onChange={(name, value) =>
              setValue(name, value, { shouldDirty: true, shouldValidate: true })
            }
          />
          <div className="grid gap-2 py-2">
            <Controller
              name="quickbar_opacity"
              control={control}
              render={({ field }) => (
                <Field>
                  <Label htmlFor="quickbar_opacity">
                    Quick bar opacity (0.5 – 1.0)
                  </Label>
                  <div className="flex items-center gap-3">
                    <input
                      id="quickbar_opacity"
                      type="range"
                      min={0.5}
                      max={1}
                      step={0.05}
                      disabled={readOnly}
                      value={field.value}
                      onChange={(event) =>
                        field.onChange(Number(event.target.value))
                      }
                      className="w-full accent-teal-700"
                      aria-describedby="quickbar_opacity_value"
                    />
                    <output
                      id="quickbar_opacity_value"
                      aria-label="Opacity value"
                      className="w-10 shrink-0 text-right text-sm tabular-nums text-neutral-600 dark:text-neutral-300"
                    >
                      {Number(field.value).toFixed(2)}
                    </output>
                  </div>
                  <FieldError message={errors.quickbar_opacity?.message} />
                </Field>
              )}
            />
          </div>
        </CardContent>
      </Card>

      <AiRewriteCard
        control={control}
        readOnly={readOnly}
        enabledOnSave={watch("ai_enabled")}
      />

      {/* Advanced fonts: collapsed by default; no font picker on first run. */}
      <details className="rounded-lg border border-neutral-200 dark:border-neutral-800">
        <summary className="cursor-pointer px-5 py-3 text-sm font-medium">
          Advanced: fonts
        </summary>
        <div className="grid gap-4 border-t border-neutral-200 p-5 sm:grid-cols-2 dark:border-neutral-800">
          <Field>
            <Label htmlFor="font_unicode">Unicode font</Label>
            <Controller
              name="font_unicode"
              control={control}
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange} disabled={readOnly}>
                  <SelectTrigger id="font_unicode">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="kalpurush">Kalpurush</SelectItem>
                    <SelectItem value="siyam-rupali">Siyam Rupali</SelectItem>
                    <SelectItem value="noto-sans-bengali">Noto Sans Bengali</SelectItem>
                  </SelectContent>
                </Select>
              )}
            />
            <FieldError message={errors.font_unicode?.message} />
          </Field>
          <Field>
            <Label htmlFor="font_bijoy">Bijoy font</Label>
            <Controller
              name="font_bijoy"
              control={control}
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange} disabled={readOnly}>
                  <SelectTrigger id="font_bijoy">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="sutonnymj">SutonnyMJ</SelectItem>
                    <SelectItem value="kalpurush-ansi">Kalpurush ANSI</SelectItem>
                  </SelectContent>
                </Select>
              )}
            />
            <FieldError message={errors.font_bijoy?.message} />
          </Field>
          <Field>
            <Label htmlFor="font_family">Custom font family (optional)</Label>
            <Input
              id="font_family"
              placeholder="Leave empty for default"
              disabled={readOnly}
              {...register("font_family")}
            />
            <FieldError message={errors.font_family?.message} />
          </Field>
        </div>
      </details>

      <div className="sticky bottom-0 flex items-center gap-2 border-t border-neutral-200 bg-white/95 py-3 backdrop-blur lg:col-span-2 dark:border-neutral-800 dark:bg-neutral-950/95">
        <Button type="submit" disabled={readOnly || isSubmitting || !isDirty}>
          {isSubmitting ? "Saving…" : "Save"}
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={readOnly || !isDirty}
          onClick={() => reset(toForm(settingsQuery.data))}
        >
          Reset
        </Button>
        {isDirty && (
          <p role="status" className="text-xs text-neutral-500 dark:text-neutral-400">
            Unsaved changes.
          </p>
        )}
        {readOnly && (
          <p role="status" className="text-xs text-neutral-500 dark:text-neutral-400">
            Read-only: daemon unavailable.
          </p>
        )}
      </div>
    </form>
  );
}
