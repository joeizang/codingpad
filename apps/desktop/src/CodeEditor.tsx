import { useEffect, useMemo, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import Editor from "./MonacoEditor";
import * as monaco from "monaco-editor/editor/editor.api.js";
import "monaco-editor/languages/definitions/python/register.js";
import "monaco-editor/languages/definitions/csharp/register.js";
import "monaco-editor/languages/definitions/sql/register.js";
// Syntax highlighting only. The TS language server arrives in M5.4.
import "monaco-editor/languages/definitions/typescript/register.js";
import { scriptLanguageDetails } from "./scriptLanguages";
import { createSqlCompletions } from "./sqlCompletions";
import { EDITOR_OPTIONS } from "./editorOptions";
import type { ConnectionSchema } from "./schema";
import type { CompileDiagnostic } from "./protocol";
import EditorWorker from "monaco-editor/editor/editor.worker.js?worker";
import {
  completionDocumentation,
  completionKind,
  completionLabel,
  csharpDirectiveKey,
  diagnosticMarkers,
  diagnosticsFor,
  hoverMarkdown,
  inlineDependencyKey,
  LSP_SESSION,
  summarizeDiagnostics,
  type DiagnosticsSummary,
  type LspCompletionItem,
  type LspCompletionList,
  type LspDiagnostic,
  type LspEvent,
  type LspHover,
  type LspRange,
} from "./lsp";

// Bundle Monaco and its worker locally; no CDN or runtime network dependency.
self.MonacoEnvironment = { getWorker: () => new EditorWorker() };

// Matches the app's dark tokens in styles.css so the editor and chrome read as one surface.
monaco.editor.defineTheme?.("codingpad-dark", {
  base: "vs-dark",
  inherit: true,
  rules: [
    { token: "comment", foreground: "6b7581", fontStyle: "italic" },
    { token: "keyword", foreground: "c49bea" },
    { token: "string", foreground: "b9d6ac" },
    { token: "number", foreground: "e2b85e" },
    { token: "type", foreground: "8fb4e8" },
    { token: "type.identifier", foreground: "8fb4e8" },
  ],
  colors: {
    "editor.background": "#15181c",
    "editor.foreground": "#dfe3e8",
    "editor.lineHighlightBackground": "#1b2026",
    "editor.lineHighlightBorder": "#00000000",
    "editorLineNumber.foreground": "#5b6571",
    "editorLineNumber.activeForeground": "#dfe3e8",
    "editorCursor.foreground": "#b9d6ac",
    "editor.selectionBackground": "#2f3d4d",
    "editorIndentGuide.background1": "#22272e",
    "editorWidget.background": "#22272e",
    "editorWidget.border": "#39414b",
    "editorSuggestWidget.background": "#22272e",
    "editorSuggestWidget.border": "#39414b",
    "editorSuggestWidget.selectedBackground": "#2b323b",
    "editorHoverWidget.background": "#22272e",
    "editorHoverWidget.border": "#39414b",
    "scrollbarSlider.background": "#2a303880",
    "scrollbarSlider.hoverBackground": "#39414b",
  },
});

export type EditorHandle = {
  revealLineInCenter: (line: number) => void;
  setPosition: (position: { lineNumber: number; column: number }) => void;
  focus: () => void;
};

type Props = {
  id: string;
  language?: string;
  path?: string;
  compileDiagnostics?: CompileDiagnostic[];
  toolchainVersion?: number;
  sqlSchema?: ConnectionSchema;
  dataRevision?: number;
  source: string;
  onChange: (source: string) => void;
  onRun: () => void;
  onMount?: (editor: EditorHandle) => void;
  onCursor?: (line: number, column: number) => void;
  onDiagnostics?: (summary: DiagnosticsSummary) => void;
};

const EMPTY_DIAGNOSTICS: CompileDiagnostic[] = [];

const COMPLETION_TRIGGERS = [".", "'", '"', "/"];

type MonacoCompletionItem = monaco.languages.CompletionItem & {
  data?: unknown;
};

function toRange(range: LspRange): monaco.Range {
  return new monaco.Range(
    range.start.line + 1,
    range.start.character + 1,
    range.end.line + 1,
    range.end.character + 1,
  );
}

function toCompletionItem(
  item: LspCompletionItem,
  fallback: monaco.Range,
): MonacoCompletionItem {
  const edit = item.textEdit;
  const range = edit
    ? "range" in edit
      ? toRange(edit.range)
      : { insert: toRange(edit.insert), replace: toRange(edit.replace) }
    : fallback;
  const documentation = completionDocumentation(item);
  return {
    label: completionLabel(item),
    tags: item.tags?.includes(1) ? [monaco.languages.CompletionItemTag.Deprecated] : undefined,
    data: item,
    kind: completionKind(item.kind) as monaco.languages.CompletionItemKind,
    detail: item.detail,
    documentation: documentation ? { value: documentation } : undefined,
    insertText: edit?.newText ?? item.insertText ?? completionLabel(item),
    insertTextRules:
      item.insertTextFormat === 2
        ? monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet
        : undefined,
    range,
    sortText: item.sortText,
    filterText: item.filterText,
    preselect: item.preselect,
    commitCharacters: item.commitCharacters,
    additionalTextEdits: item.additionalTextEdits?.map((additional) => ({
      range: toRange(additional.range),
      text: additional.newText,
    })),
  };
}

const directiveKey = (source: string, language: string) =>
  language === "csharp" ? csharpDirectiveKey(source) : inlineDependencyKey(source);

const supportsCodeIntelligence = (language: string) =>
  language === "python" || language === "csharp";

function attachedConnection(source: string): string {
  const first = source.split(/\r?\n/, 1)[0] ?? "";
  const header = first.match(/^\s*(?:#|\/\/|--)\s*codingpad:\s*(.*)$/);
  if (!header) return "";
  const setting = header[1].match(/(?:^|\s)connection=(\"[^\"]*\"|'[^']*'|\S+)/);
  return setting ? setting[1].replace(/^[\"']|[\"']$/g, "") : "";
}

export default function CodeEditor({
  id,
  language = "python",
  path,
  compileDiagnostics = EMPTY_DIAGNOSTICS,
  toolchainVersion = 0,
  sqlSchema,
  dataRevision = 0,
  source,
  onChange,
  onRun,
  onMount,
  onCursor,
  onDiagnostics,
}: Props) {
  const onCursorRef = useRef(onCursor);
  onCursorRef.current = onCursor;
  const completeSql = useMemo(() => createSqlCompletions(sqlSchema), [sqlSchema]);
  const completeSqlRef = useRef(completeSql);
  completeSqlRef.current = completeSql;
  const generation = useRef(0);
  const [interactive, setInteractive] = useState(false);
  const connectionKey = attachedConnection(source);
  const editorRef = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const lsp = useRef<{ id: string; uri: string; ready: boolean } | null>(null);
  const pendingDiagnostics = useRef<LspDiagnostic[] | null>(null);
  const pendingChange = useRef<string | null>(null);
  const changeTimer = useRef<number | null>(null);
  const seenDiagnostics = useRef(false);
  const onDiagnosticsRef = useRef(onDiagnostics);
  const cleanup = useRef<(() => void) | null>(null);

  onDiagnosticsRef.current = onDiagnostics;

  const flushChange = async () => {
    if (changeTimer.current !== null) {
      window.clearTimeout(changeTimer.current);
      changeTimer.current = null;
    }
    const value = pendingChange.current;
    const endpoint = lsp.current;
    if (value === null || !endpoint?.ready) return;
    pendingChange.current = null;
    await invoke("change_language_server", {
      scriptId: endpoint.id,
      source: value,
    }).catch(() => {});
  };

  const [dependencyKey, setDependencyKey] = useState(() =>
    directiveKey(source, language),
  );

  useEffect(() => {
    const next = directiveKey(source, language);
    if (next === dependencyKey) return;
    const timer = window.setTimeout(() => setDependencyKey(next), 1000);
    return () => window.clearTimeout(timer);
  }, [source, dependencyKey, language]);

  const applyDiagnostics = (diagnostics: LspDiagnostic[]) => {
    onDiagnosticsRef.current?.(summarizeDiagnostics(diagnostics));
    const model = editorRef.current?.getModel();
    if (!model) {
      pendingDiagnostics.current = diagnostics;
      return;
    }
    monaco.editor.setModelMarkers(
      model,
      "codingpad-lsp",
      diagnosticMarkers(
        diagnostics,
        language === "csharp" ? "csharp" : "basedpyright",
      ) as monaco.editor.IMarkerData[],
    );
  };

  const applyCompileDiagnostics = () => {
    const model = editorRef.current?.getModel();
    if (model) {
      monaco.editor.setModelMarkers(
        model,
        "codingpad-compiler",
        compileDiagnostics.map((diagnostic) => ({
          severity: monaco.MarkerSeverity.Error,
          message: diagnostic.message,
          startLineNumber: diagnostic.line,
          startColumn: diagnostic.column,
          endLineNumber: diagnostic.endLine,
          endColumn: diagnostic.endLine === diagnostic.line
            ? Math.max(diagnostic.column + 1, diagnostic.endColumn)
            : diagnostic.endColumn,
        })),
      );
    }
    onDiagnosticsRef.current?.({
      errors: compileDiagnostics.length,
      warnings: 0,
      message: compileDiagnostics.length ? undefined : "Compile diagnostics on Run",
    });
  };

  useEffect(() => {
    if (language === "csharp") applyCompileDiagnostics();
  }, [language, compileDiagnostics]);

  useEffect(() => {
    if (!interactive || language !== "typescript") return;
    let stopped = false;
    const timer = window.setTimeout(() => {
      void invoke("begin_language_server_session", { session: LSP_SESSION })
        .then(() => {
          if (stopped) return;
          return invoke("prepare_typescript_project", { scriptId: id, source, session: LSP_SESSION });
        })
        .catch(() => undefined); // Run reports package preparation errors in Results.
    }, 400);
    return () => {
      stopped = true;
      window.clearTimeout(timer);
    };
  }, [id, source, language, interactive]);

  useEffect(() => {
    // Let the editor paint before starting the background language server.
    if (!interactive) return;
    if (!supportsCodeIntelligence(language)) {
      onDiagnosticsRef.current?.({
        errors: 0,
        warnings: 0,
        message: "Code intelligence unavailable",
      });
      return;
    }
    const current = ++generation.current;
    let stopped = false;
    const initial = source;
    pendingChange.current = null;
    seenDiagnostics.current = false;
    pendingDiagnostics.current = null;
    lsp.current = null;

    const channel = new Channel<LspEvent>();
    channel.onmessage = (event) => {
      if (stopped || current !== generation.current) return;
      if (event.method !== "textDocument/publishDiagnostics") return;
      const params = event.params as { uri: string };
      if (lsp.current && params.uri !== lsp.current.uri) return;
      seenDiagnostics.current = true;
      applyDiagnostics(diagnosticsFor(event));
    };

    onDiagnosticsRef.current?.({
      errors: 0,
      warnings: 0,
      message: "Starting code intelligence…",
    });
    invoke("begin_language_server_session", { session: LSP_SESSION })
      .catch(() => undefined)
      .then(() => {
        if (stopped || current !== generation.current) return null;
        return invoke<{ uri: string }>("start_language_server", {
          scriptId: id,
          source: initial,
          language,
          path: path ?? null,
          session: LSP_SESSION,
          onEvent: channel,
        });
      })
      .then((result) => {
        if (!result || stopped || current !== generation.current) return;
        lsp.current = { id, uri: result.uri, ready: true };
        if (!seenDiagnostics.current) {
          onDiagnosticsRef.current?.({
            errors: 0,
            warnings: 0,
            message: "Checking types…",
          });
        }
        void flushChange();
      })
      .catch((error) => {
        if (stopped || current !== generation.current) return;
        onDiagnosticsRef.current?.({
          errors: 0,
          warnings: 0,
          message: `Code intelligence unavailable: ${String(error)}`,
        });
      });

    return () => {
      stopped = true;
      lsp.current = null;
      if (changeTimer.current !== null) {
        window.clearTimeout(changeTimer.current);
        changeTimer.current = null;
      }
      pendingChange.current = null;
      const model = editorRef.current?.getModel();
      if (model) monaco.editor.setModelMarkers(model, "codingpad-lsp", []);
    };
  }, [
    id,
    dependencyKey,
    toolchainVersion,
    dataRevision,
    connectionKey,
    language,
    path,
    interactive,
  ]);

  useEffect(
    () => () => {
      cleanup.current?.();
      cleanup.current = null;
      editorRef.current = null;
    },
    [],
  );

  return (
    <Editor
      path={`${id}.${scriptLanguageDetails(language)?.extension ?? "txt"}`}
      language={language}
      theme="codingpad-dark"
      value={source}
      onChange={(value) => onChange(value ?? "")}
      onMount={(editor) => {
        editorRef.current = editor;
        const position = editor.getPosition?.();
        if (position) onCursorRef.current?.(position.lineNumber, position.column);
        editor.onDidChangeCursorPosition?.((event) =>
          onCursorRef.current?.(event.position.lineNumber, event.position.column),
        );
        const reportReady = () => {
          // Readiness means the mounted editor can receive input, with its run
          // command and completion/hover providers installed, after a paint.
          const mounted = performance.now();
          requestAnimationFrame(() => requestAnimationFrame(() => {
            if (editorRef.current !== editor || !editor.getModel() ||
                !editor.getDomNode()?.isConnected ||
                editor.getOption(monaco.editor.EditorOption.readOnly)) return;
            setInteractive(true);
            void invoke("ui_ready", { frontend: {
              timeOrigin: performance.timeOrigin,
              evaluated: performance.getEntriesByName("frontend-evaluated")[0]?.startTime,
              createStarted: performance.getEntriesByName("editor-create-started").at(-1)?.startTime,
              editorMounted: mounted,
              interactive: performance.now(),
            } }).catch(() => {});
          }));
        };
        editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, onRun);
        if (language === "sql") {
          const completion = monaco.languages.registerCompletionItemProvider("sql", {
            triggerCharacters: ["."],
            provideCompletionItems: (model, position) => {
              if (model !== editor.getModel()) return { suggestions: [] };
              const source = model.getValue(), offset = model.getOffsetAt(position);
              const result = completeSqlRef.current(source, offset);
              // Monaco filters from the replacement range, including a typed quote.
              const first = source[result.start];
              const opener = result.start < offset && (first === '"' || first === "[" || first === "`") ? first : "";
              const start = model.getPositionAt(result.start), end = model.getPositionAt(result.end);
              const range = new monaco.Range(start.lineNumber, start.column, end.lineNumber, end.column);
              return {
                suggestions: result.suggestions.map((item) => ({ ...item, range,
                  filterText: opener + item.label,
                  kind: item.kind === "table" ? monaco.languages.CompletionItemKind.Struct
                    : item.kind === "column" ? monaco.languages.CompletionItemKind.Field
                    : monaco.languages.CompletionItemKind.Keyword,
                })),
                incomplete: result.incomplete,
              };
            },
          });
          cleanup.current = () => completion.dispose();
        }
        if (!supportsCodeIntelligence(language)) {
          onMount?.(editor);
          reportReady();
          return;
        }
        if (language === "csharp") applyCompileDiagnostics();
        if (pendingDiagnostics.current) {
          const model = editor.getModel();
          if (model) {
            monaco.editor.setModelMarkers(
              model,
              "codingpad-lsp",
              diagnosticMarkers(pendingDiagnostics.current) as monaco.editor.IMarkerData[],
            );
          }
          pendingDiagnostics.current = null;
        }

        const change = editor.onDidChangeModelContent(() => {
          pendingChange.current = editor.getValue();
          if (changeTimer.current !== null) {
            window.clearTimeout(changeTimer.current);
          }
          changeTimer.current = window.setTimeout(() => {
            void flushChange();
          }, 120);
        });

        const completion = monaco.languages.registerCompletionItemProvider(
          language,
          {
            triggerCharacters: COMPLETION_TRIGGERS,
            provideCompletionItems: async (model, position) => {
              const endpoint = lsp.current;
              if (!endpoint) return { suggestions: [] };
              await flushChange();
              const result = await invoke<
                LspCompletionList | LspCompletionItem[]
              >("request_language_server", {
                scriptId: endpoint.id,
                method: "textDocument/completion",
                params: {
                  textDocument: { uri: endpoint.uri },
                  position: {
                    line: position.lineNumber - 1,
                    character: position.column - 1,
                  },
                  context: { triggerKind: 1 },
                },
              }).catch(() => null);
              if (!result) return { suggestions: [] };
              const items = Array.isArray(result)
                ? result
                : (result.items ?? []);
              const incomplete = !Array.isArray(result) && !!result.isIncomplete;
              const word = model.getWordUntilPosition(position);
              const fallback = new monaco.Range(
                position.lineNumber,
                word.startColumn,
                position.lineNumber,
                word.endColumn,
              );
              return {
                suggestions: items.map((item) =>
                  toCompletionItem(item, fallback),
                ),
                incomplete,
              };
            },
            resolveCompletionItem: async (item) => {
              const endpoint = lsp.current;
              const data = (item as MonacoCompletionItem).data;
              if (!endpoint || data === undefined) return item;
              const resolved = await invoke<LspCompletionItem>(
                "request_language_server",
                {
                  scriptId: endpoint.id,
                  method: "completionItem/resolve",
                  params: data,
                },
              ).catch(() => null);
              if (!resolved) return item;
              const documentation = completionDocumentation(resolved);
              if (documentation) item.documentation = { value: documentation };
              if (resolved.detail) item.detail = resolved.detail;
              if (resolved.tags) item.tags = resolved.tags.includes(1)
                ? [monaco.languages.CompletionItemTag.Deprecated] : [];
              if (resolved.additionalTextEdits) {
                item.additionalTextEdits = resolved.additionalTextEdits.map(
                  (additional) => ({
                    range: toRange(additional.range),
                    text: additional.newText,
                  }),
                );
              }
              return item;
            },
          },
        );

        const hover = monaco.languages.registerHoverProvider(language, {
          provideHover: async (_model, position) => {
            const endpoint = lsp.current;
            if (!endpoint) return null;
            await flushChange();
            const result = await invoke<LspHover | null>(
              "request_language_server",
              {
                scriptId: endpoint.id,
                method: "textDocument/hover",
                params: {
                  textDocument: { uri: endpoint.uri },
                  position: {
                    line: position.lineNumber - 1,
                    character: position.column - 1,
                  },
                },
              },
            ).catch(() => null);
            const contents = hoverMarkdown(result);
            if (!contents) return null;
            return { contents: [{ value: contents }] };
          },
        });

        cleanup.current?.();
        cleanup.current = () => {
          change.dispose();
          completion.dispose();
          hover.dispose();
        };
        onMount?.(editor);
        reportReady();
      }}
      options={EDITOR_OPTIONS}
    />
  );
}
