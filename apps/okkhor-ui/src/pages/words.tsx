import * as React from "react";
import { useMutation, useQuery, useQueryClient, keepPreviousData } from "@tanstack/react-query";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { toast } from "sonner";
import { api } from "../lib/ipc";
import { describeError } from "../lib/errors";
import {
  clampPage,
  downloadTextFile,
  previewImport,
  totalPages,
  wordFormSchema,
  type ImportFormat,
  type Word,
  type WordForm,
} from "../lib/words";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "../components/ui/card";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "../components/ui/dialog";
import { Field, FieldError, Label } from "../components/ui/field";
import { Input } from "../components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";

export default function WordsPage({ readOnly }: { readOnly: boolean }) {
  const queryClient = useQueryClient();
  const settingsQuery = useQuery({
    queryKey: ["settings"],
    queryFn: () => api.getSettings(),
    retry: false,
    refetchOnWindowFocus: false,
    staleTime: 10_000,
  });
  const pageSize = settingsQuery.data?.word_manager_page_size ?? 50;

  const [searchInput, setSearchInput] = React.useState("");
  const [search, setSearch] = React.useState<string | null>(null);
  const [page, setPage] = React.useState(0);

  const wordsQuery = useQuery({
    queryKey: ["words", search, page, pageSize],
    queryFn: () => api.listWords(search, page * pageSize, pageSize),
    retry: false,
    refetchOnWindowFocus: false,
    placeholderData: keepPreviousData,
  });

  const invalidateWords = React.useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: ["words"] });
    void queryClient.invalidateQueries({ queryKey: ["status"] });
  }, [queryClient]);

  const total = wordsQuery.data?.total ?? 0;
  const pages = totalPages(total, pageSize);
  const safePage = clampPage(page, total, pageSize);
  const words = wordsQuery.data?.words ?? [];

  // --- add form ---
  const addForm = useForm<WordForm>({
    resolver: zodResolver(wordFormSchema),
    defaultValues: { roman: "", output: "", language: "bn" },
  });
  const [addRemoteError, setAddRemoteError] = React.useState<string | null>(null);
  const addMutation = useMutation({
    mutationFn: (values: WordForm) => api.addWord(values.roman, values.output, values.language),
    onSuccess: () => {
      addForm.reset({ roman: "", output: "", language: "bn" });
      setAddRemoteError(null);
      invalidateWords();
      toast.success("Word added.");
    },
    onError: (error) => {
      // Inline static text only; never the submitted word.
      setAddRemoteError(describeError(error));
    },
  });

  // --- remove with confirm ---
  const [pendingRemove, setPendingRemove] = React.useState<Word | null>(null);
  const removeMutation = useMutation({
    mutationFn: (id: number) => api.removeWord(id),
    onSuccess: () => {
      setPendingRemove(null);
      // If the last row on this page was removed, step back so the table
      // never strands on an empty page.
      if (words.length === 1 && page > 0) {
        setPage(page - 1);
      }
      invalidateWords();
      toast.success("Word removed.");
    },
    onError: (error) => toast.error(describeError(error)),
  });

  // --- import with preview ---
  const [importFormat, setImportFormat] = React.useState<ImportFormat>("csv");
  const [importText, setImportText] = React.useState("");
  const [importRemoteError, setImportRemoteError] = React.useState<string | null>(null);
  const preview = React.useMemo(
    () => (importText.trim() === "" ? null : previewImport(importText, importFormat)),
    [importText, importFormat],
  );
  const importMutation = useMutation({
    mutationFn: () => api.importWords(importFormat, importText),
    onSuccess: (result) => {
      setImportRemoteError(null);
      invalidateWords();
      toast.success(`Imported ${result.imported}, skipped ${result.skipped}.`);
    },
    onError: (error) => setImportRemoteError(describeError(error)),
  });

  const readImportFile = React.useCallback((file: File | undefined) => {
    if (!file) {
      return;
    }
    const reader = new FileReader();
    reader.onload = () => {
      setImportText(typeof reader.result === "string" ? reader.result : "");
      setImportRemoteError(null);
    };
    reader.onerror = () => {
      setImportRemoteError("Could not read that file.");
    };
    reader.readAsText(file);
  }, []);

  // --- export via blob download (no dialog plugin needed) ---
  const [exportFormat, setExportFormat] = React.useState<ImportFormat>("csv");
  const [exportError, setExportError] = React.useState<string | null>(null);
  const exportMutation = useMutation({
    mutationFn: () => api.exportWords(exportFormat),
    onSuccess: (result) => {
      setExportError(null);
      downloadTextFile(
        `okkhor-words.${result.format === "json" ? "json" : "csv"}`,
        result.data,
        result.format === "json" ? "application/json" : "text/csv",
      );
      toast.success("Export downloaded.");
    },
    onError: (error) => setExportError(describeError(error)),
  });

  return (
    <div data-testid="page-words" className="grid min-w-0 items-start gap-4 xl:grid-cols-5">
      <Card className="min-w-0 xl:col-span-3">
        <CardHeader>
          <CardTitle>Word manager</CardTitle>
          <CardDescription>
            Personal words stored encrypted by the daemon. Add, search, remove,
            import, and export — words are never written to logs.
          </CardDescription>
        </CardHeader>
        <CardContent className="grid gap-3">
          {readOnly && (
            <p role="alert" className="rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100">
              The daemon is not running, so the word list is read-only. Start
              Okkhor and retry.
            </p>
          )}
          <form
            aria-label="Search words"
            className="flex gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              setSearch(searchInput.trim() === "" ? null : searchInput.trim());
              setPage(0);
            }}
          >
            <Field className="flex-1">
              <Label htmlFor="words-search">Search</Label>
              <Input
                id="words-search"
                type="search"
                placeholder="roman or output…"
                value={searchInput}
                disabled={readOnly}
                onChange={(event) => setSearchInput(event.target.value)}
              />
            </Field>
            <div className="flex items-end">
              <Button type="submit" variant="outline" disabled={readOnly}>
                Search
              </Button>
            </div>
          </form>

          {wordsQuery.isPending ? (
            <div aria-label="Loading words" className="grid animate-pulse gap-2">
              {[0, 1, 2].map((key) => (
                <div key={key} className="h-10 rounded bg-neutral-100 dark:bg-neutral-800" />
              ))}
            </div>
          ) : wordsQuery.isError ? (
            <div className="grid gap-2">
              <p role="alert" className="text-sm text-neutral-600 dark:text-neutral-300">
                {describeError(wordsQuery.error)}
              </p>
              <div>
                <Button variant="outline" onClick={() => void wordsQuery.refetch()} aria-label="Retry loading words">
                  Retry
                </Button>
              </div>
            </div>
          ) : (
            <div className="grid gap-2">
              <div className="overflow-x-auto rounded-md border border-neutral-200 dark:border-neutral-800">
                <table className="w-full text-left text-sm">
                  <caption className="px-3 py-2 text-left text-xs text-neutral-500 dark:text-neutral-400">
                    {total} {total === 1 ? "word" : "words"} stored
                  </caption>
                  <thead>
                    <tr className="border-b border-neutral-200 bg-neutral-50 dark:border-neutral-800 dark:bg-neutral-900">
                      <th scope="col" className="px-3 py-2 font-medium">Roman</th>
                      <th scope="col" className="px-3 py-2 font-medium">Output</th>
                      <th scope="col" className="px-3 py-2 font-medium">Language</th>
                      <th scope="col" className="px-3 py-2 font-medium">Source</th>
                      <th scope="col" className="px-3 py-2 font-medium">
                        <span className="sr-only">Actions</span>
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {words.length === 0 ? (
                      <tr>
                        <td colSpan={5} className="px-3 py-4 text-center text-neutral-500 dark:text-neutral-400">
                          No words found.
                        </td>
                      </tr>
                    ) : (
                      words.map((word) => (
                        <tr
                          key={word.id}
                          className="border-b border-neutral-100 transition-colors last:border-0 hover:bg-neutral-50 dark:border-neutral-800 dark:hover:bg-neutral-800/50"
                        >
                          <td className="px-3 py-2">{word.roman}</td>
                          <td className="px-3 py-2 text-lg">{word.output}</td>
                          <td className="px-3 py-2">{word.language}</td>
                          <td className="px-3 py-2">{word.source}</td>
                          <td className="px-3 py-2 text-right">
                            <Button
                              type="button"
                              variant="outline"
                              size="sm"
                              aria-label={`Remove word ${word.id}`}
                              disabled={readOnly || removeMutation.isPending}
                              onClick={() => setPendingRemove(word)}
                            >
                              Remove
                            </Button>
                          </td>
                        </tr>
                      ))
                    )}
                  </tbody>
                </table>
              </div>
              <div className="flex items-center justify-between gap-2">
                <p role="status" className="text-xs text-neutral-500 dark:text-neutral-400" aria-live="polite">
                  Page {safePage + 1} of {pages}
                </p>
                <div className="flex gap-2">
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled={readOnly || safePage <= 0 || wordsQuery.isFetching}
                    onClick={() => setPage(safePage - 1)}
                    aria-label="Previous word page"
                  >
                    Previous
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled={readOnly || safePage >= pages - 1 || wordsQuery.isFetching}
                    onClick={() => setPage(safePage + 1)}
                    aria-label="Next word page"
                  >
                    Next
                  </Button>
                </div>
              </div>
            </div>
          )}
        </CardContent>
      </Card>

      {/* Right rail at xl+: add + import + export stack beside the table. */}
      <div className="grid min-w-0 gap-4 xl:col-span-2">
      <Card>
        <CardHeader>
          <CardTitle>Add a word</CardTitle>
        </CardHeader>
        <CardContent>
          <form
            aria-label="Add word"
            className="grid gap-3 sm:grid-cols-2 xl:grid-cols-1"
            onSubmit={(event) => void addForm.handleSubmit((values) => addMutation.mutate(values))(event)}
          >
            <Field>
              <Label htmlFor="word-roman">Roman (1–64 chars)</Label>
              <Input id="word-roman" disabled={readOnly} {...addForm.register("roman")} />
              <FieldError message={addForm.formState.errors.roman?.message} />
            </Field>
            <Field>
              <Label htmlFor="word-output">Output (1–128 chars)</Label>
              <Input id="word-output" disabled={readOnly} {...addForm.register("output")} />
              <FieldError message={addForm.formState.errors.output?.message} />
            </Field>
            <Field>
              <Label htmlFor="word-language">Language</Label>
              <Select
                value={addForm.watch("language")}
                onValueChange={(value: "bn" | "en") =>
                  addForm.setValue("language", value, { shouldValidate: true })
                }
                disabled={readOnly}
              >
                <SelectTrigger id="word-language">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="bn">Bangla (bn)</SelectItem>
                  <SelectItem value="en">English (en)</SelectItem>
                </SelectContent>
              </Select>
              <FieldError message={addForm.formState.errors.language?.message} />
            </Field>
            <div className="flex items-end">
              <Button type="submit" disabled={readOnly || addMutation.isPending}>
                {addMutation.isPending ? "Adding…" : "Add word"}
              </Button>
            </div>
            {addRemoteError !== null && (
              <p role="alert" className="text-sm text-red-700 sm:col-span-2 dark:text-red-400">
                {addRemoteError}
              </p>
            )}
          </form>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Import words</CardTitle>
          <CardDescription>
            CSV with header <code>roman,output,language</code> or JSON. The
            table below previews the first 20 rows; counts are estimates — the
            daemon re-validates on import.
          </CardDescription>
        </CardHeader>
        <CardContent className="grid gap-3">
          <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-1">
            <Field>
              <Label htmlFor="import-format">Format</Label>
              <Select
                value={importFormat}
                onValueChange={(value: ImportFormat) => setImportFormat(value)}
                disabled={readOnly}
              >
                <SelectTrigger id="import-format">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="csv">CSV</SelectItem>
                  <SelectItem value="json">JSON</SelectItem>
                </SelectContent>
              </Select>
            </Field>
            <Field>
              <Label htmlFor="import-file">File</Label>
              <Input
                id="import-file"
                type="file"
                accept={importFormat === "csv" ? ".csv,text/csv" : ".json,application/json"}
                disabled={readOnly}
                onChange={(event) => readImportFile(event.target.files?.[0])}
              />
            </Field>
          </div>
          {preview !== null && (
            <div className="grid gap-2">
              {preview.fatal !== null ? (
                <p role="alert" className="text-sm text-red-700 dark:text-red-400">
                  {preview.fatal}
                </p>
              ) : (
                <>
                  <p role="status" className="text-xs text-neutral-500 dark:text-neutral-400">
                    Showing {preview.rows.length} of {preview.totalRows} rows ·{" "}
                    {preview.imported} new, {preview.skipped} skipped (estimated).
                  </p>
                  <div className="overflow-x-auto rounded-md border border-neutral-200 dark:border-neutral-800">
                    <table className="w-full text-left text-sm">
                      <thead>
                        <tr className="border-b border-neutral-200 bg-neutral-50 dark:border-neutral-800 dark:bg-neutral-900">
                          <th scope="col" className="px-3 py-2 font-medium">Roman</th>
                          <th scope="col" className="px-3 py-2 font-medium">Output</th>
                          <th scope="col" className="px-3 py-2 font-medium">Language</th>
                          <th scope="col" className="px-3 py-2 font-medium">Status</th>
                        </tr>
                      </thead>
                      <tbody>
                        {preview.rows.map((row, index) => (
                          <tr
                            key={index}
                            className="border-b border-neutral-100 transition-colors last:border-0 hover:bg-neutral-50 dark:border-neutral-800 dark:hover:bg-neutral-800/50"
                          >
                            <td className="px-3 py-2">{row.roman}</td>
                            <td className="px-3 py-2 text-lg">{row.output}</td>
                            <td className="px-3 py-2">{row.language}</td>
                            <td className="px-3 py-2">
                              {row.error === null ? (
                                <span className="text-green-700 dark:text-green-400">Ready</span>
                              ) : (
                                <span className="text-red-700 dark:text-red-400">{row.error}</span>
                              )}
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                </>
              )}
            </div>
          )}
          {importRemoteError !== null && (
            <p role="alert" className="text-sm text-red-700 dark:text-red-400">
              {importRemoteError}
            </p>
          )}
          <div>
            <Button
              type="button"
              disabled={
                readOnly ||
                importMutation.isPending ||
                preview === null ||
                preview.fatal !== null ||
                preview.imported === 0
              }
              onClick={() => importMutation.mutate()}
            >
              {importMutation.isPending ? "Importing…" : "Import"}
            </Button>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Export words</CardTitle>
          <CardDescription>Downloads the current list as a file.</CardDescription>
        </CardHeader>
        <CardContent className="grid gap-3">
          <div className="flex flex-wrap items-end gap-2">
            <Field className="min-w-40">
              <Label htmlFor="export-format">Format</Label>
              <Select
                value={exportFormat}
                onValueChange={(value: ImportFormat) => setExportFormat(value)}
                disabled={readOnly}
              >
                <SelectTrigger id="export-format">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="csv">CSV</SelectItem>
                  <SelectItem value="json">JSON</SelectItem>
                </SelectContent>
              </Select>
            </Field>
            <Button
              type="button"
              variant="outline"
              disabled={readOnly || exportMutation.isPending}
              onClick={() => exportMutation.mutate()}
            >
              {exportMutation.isPending ? "Exporting…" : "Download export"}
            </Button>
          </div>
          {exportError !== null && (
            <p role="alert" className="text-sm text-red-700 dark:text-red-400">
              {exportError}
            </p>
          )}
        </CardContent>
      </Card>
      </div>

      <Dialog open={pendingRemove !== null} onOpenChange={(open) => { if (!open) setPendingRemove(null); }}>
        <DialogContent aria-label="Confirm word removal">
          <DialogHeader>
            <DialogTitle>Remove this word?</DialogTitle>
          </DialogHeader>
          <p className="text-sm text-neutral-600 dark:text-neutral-300">
            This removes the selected word from the encrypted store. This cannot
            be undone.
          </p>
          <div className="flex justify-end gap-2">
            <Button type="button" variant="outline" onClick={() => setPendingRemove(null)}>
              Cancel
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={removeMutation.isPending}
              onClick={() => {
                if (pendingRemove !== null) {
                  removeMutation.mutate(pendingRemove.id);
                }
              }}
            >
              {removeMutation.isPending ? "Removing…" : "Remove"}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
