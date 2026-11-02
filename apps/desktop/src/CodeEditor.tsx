import { useEffect, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import Editor, { loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor/editor/editor.api.js";
import "monaco-editor/languages/definitions/python/register.js";
import EditorWorker from "monaco-editor/editor/editor.worker.js?worker";
import {
  completionDocumentation,
  completionKind,
  completionLabel,
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
loader.config({ monaco });

export type EditorHandle = {
  revealLineInCenter: (line: number) => void;
  setPosition: (position: { lineNumber: number; column: number }) => void;
  focus: () => void;
};

type Props = {
  id: string;
  toolchainVersion?: number;
  source: string;
  onChange: (source: string) => void;
  onRun: () => void;
  onMount?: (editor: EditorHandle) => void;
  onDiagnostics?: (summary: DiagnosticsSummary) => void;
};

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

export default function CodeEditor({
  id,
  toolchainVersion = 0,
  source,
  onChange,
  onRun,
  onMount,
  onDiagnostics,
}: Props) {
  const generation = useRef(0);
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
    inlineDependencyKey(source),
  );

  useEffect(() => {
    const next = inlineDependencyKey(source);
    if (next === dependencyKey) return;
    const timer = window.setTimeout(() => setDependencyKey(next), 1000);
    return () => window.clearTimeout(timer);
  }, [source, dependencyKey]);

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
      diagnosticMarkers(diagnostics) as monaco.editor.IMarkerData[],
    );
  };

  useEffect(() => {
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
  }, [id, dependencyKey, toolchainVersion]);

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
      path={`${id}.py`}
      language="python"
      theme="vs-dark"
      value={source}
      onChange={(value) => onChange(value ?? "")}
      onMount={(editor) => {
        editorRef.current = editor;
        editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, onRun);
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
          "python",
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

        const hover = monaco.languages.registerHoverProvider("python", {
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
      }}
      options={{
        fontSize: 14,
        fontFamily: "'SFMono-Regular', Consolas, monospace",
        minimap: { enabled: false },
        automaticLayout: true,
        scrollBeyondLastLine: false,
        padding: { top: 20 },
        tabSize: 4,
        quickSuggestions: { other: true, comments: false, strings: false },
        wordBasedSuggestions: "off",
        suggestOnTriggerCharacters: true,
      }}
      loading={<p className="empty">Loading editor…</p>}
    />
  );
}
