import { act, render, waitFor } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import CodeEditor from "./CodeEditor";
// @ts-expect-error Monaco's internal filter has no public type declarations.
import { fuzzyScore } from "monaco-editor/base/common/filters.js";

const { invoke, mounted, completionProvider } = vi.hoisted(() => ({
  invoke: vi.fn(),
  mounted: { callback: undefined as undefined | ((editor: unknown) => void) },
  completionProvider: { dispose: vi.fn(), current: undefined as undefined | { resolveCompletionItem: (item: unknown) => Promise<{ tags?: number[] }>; provideCompletionItems: (model: unknown, position: unknown) => { suggestions: {label: string; insertText: string; filterText?: string}[] }; } },
}));
const disposable = () => ({ dispose: vi.fn() });
function mountEditor() {
  act(() => mounted.callback?.({
    addCommand: vi.fn(), getModel: () => ({}),
    getDomNode: () => document.body, getOption: () => false,
    onDidChangeModelContent: disposable,
  }));
}
vi.mock("@tauri-apps/api/core", () => ({
  invoke,
  Channel: class { onmessage = () => {}; },
}));
vi.mock("./MonacoEditor", () => ({
  default: ({ onMount }: { onMount: (editor: unknown) => void }) => {
    mounted.callback = onMount;
    return <div />;
  },
}));
vi.mock("monaco-editor/editor/editor.api.js", () => ({
  editor: { setModelMarkers: vi.fn(), EditorOption: { readOnly: 1 } },
  KeyMod: { CtrlCmd: 1 }, KeyCode: { Enter: 1 },
  Range: class {},
  languages: {
    CompletionItemTag: { Deprecated: 1 },
    CompletionItemKind: { Struct: 22, Field: 5, Keyword: 17 },
    registerCompletionItemProvider: (_: string, provider: unknown) => {
      completionProvider.current = provider as typeof completionProvider.current;
      return { dispose: completionProvider.dispose };
    },
    registerHoverProvider: () => ({ dispose: vi.fn() }),
  },
  MarkerSeverity: { Error: 8 },
}));
vi.mock("monaco-editor/languages/definitions/python/register.js", () => ({}));
vi.mock("monaco-editor/languages/definitions/csharp/register.js", () => ({}));
vi.mock("monaco-editor/languages/definitions/sql/register.js", () => ({}));
vi.mock("monaco-editor/editor/editor.worker.js?worker", () => ({ default: class {} }));

test("retries a failed language server after uv installation using the latest source", async () => {
  let installed = false;
  invoke.mockImplementation(async (command) => {
    if (command === "start_language_server") {
      if (!installed) throw "uv was not found";
      return { uri: "file:///script.py" };
    }
  });
  const diagnostics = vi.fn();
  const props = { id: "one", source: "41", onChange: vi.fn(), onRun: vi.fn(), onDiagnostics: diagnostics };
  const view = render(<CodeEditor {...props} toolchainVersion={0} />);
  expect(invoke).not.toHaveBeenCalledWith("start_language_server", expect.anything());
  mountEditor();
  await waitFor(() => expect(diagnostics).toHaveBeenCalledWith(expect.objectContaining({
    message: expect.stringContaining("uv was not found"),
  })));
  installed = true;
  view.rerender(<CodeEditor {...props} source="42" toolchainVersion={1} />);
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_language_server", expect.objectContaining({ scriptId: "one", source: "42" })));
  expect(invoke.mock.calls.filter(([command]) => command === "start_language_server")).toHaveLength(2);
  await waitFor(() => expect(diagnostics).toHaveBeenLastCalledWith(expect.objectContaining({ message: "Checking types…" })));
});


test("C# reports compiler diagnostics and starts Roslyn for the same script", async () => {
  invoke.mockClear();
  invoke.mockImplementation(async (command) =>
    command === "start_language_server" ? { uri: "file:///Program.cs" } : undefined,
  );
  const diagnostics = vi.fn();
  const props = { id: "csharp", language: "csharp", source: "missing();", onChange: vi.fn(), onRun: vi.fn(), onDiagnostics: diagnostics };
  const view = render(<CodeEditor {...props} compileDiagnostics={[{ message: "CS0103", line: 1, column: 1, endLine: 1, endColumn: 8 }]} />);
  mountEditor();
  await waitFor(() => expect(diagnostics).toHaveBeenCalledWith(expect.objectContaining({ errors: 1 })));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith(
      "start_language_server",
      expect.objectContaining({ scriptId: "csharp", language: "csharp", path: null }),
    ),
  );
  view.rerender(<CodeEditor {...props} source="42.Dump();" />);
  await waitFor(() => expect(diagnostics).toHaveBeenCalledWith(expect.objectContaining({ errors: 0 })));
});


test("reports editor readiness before starting intelligence and preserves resolved deprecation tags", async () => {
  invoke.mockClear();
  invoke.mockImplementation(async (command) => {
    if (command === "start_language_server") return { uri: "file:///script.py" };
    if (command === "request_language_server") return { label: "old", tags: [1] };
  });
  render(<CodeEditor id="tags" source="old" onChange={vi.fn()} onRun={vi.fn()} />);
  expect(invoke).not.toHaveBeenCalled();
  mountEditor();
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_language_server", expect.anything()));
  const commands = invoke.mock.calls.map(([command]) => command);
  expect(commands.indexOf("ui_ready")).toBeLessThan(commands.indexOf("start_language_server"));
  const resolved = await completionProvider.current!.resolveCompletionItem({ label: "old", data: {} });
  expect(resolved.tags).toEqual([1]);
});


test("SQL provider reads live snapshots locally, isolates its model, and disposes on unmount", () => {
  invoke.mockClear();
  completionProvider.dispose.mockClear();
  const props = { id: "sql", language: "sql", source: "SELECT u. FROM main.users u", onChange: vi.fn(), onRun: vi.fn() };
  const schema = { kind: "sqlite" as const, tables: [{ schema: "main", name: "users", columns: [{name:"email",dataType:"TEXT",references:[]}]}] };
  const view = render(<CodeEditor {...props} sqlSchema={schema} />);
  const model = { getValue: () => props.source, getOffsetAt: () => 9, getPositionAt: (offset: number) => ({lineNumber:1,column:offset+1}) };
  act(() => mounted.callback?.({ addCommand: vi.fn(), getModel: () => model, getDomNode: () => document.body, getOption: () => false }));
  const provider = completionProvider.current!;
  expect(provider.provideCompletionItems(model, {} as never).suggestions.map((s) => s.label)).toEqual(["email"]);
  expect(provider.provideCompletionItems({}, {} as never).suggestions).toEqual([]);
  view.rerender(<CodeEditor {...props} sqlSchema={{ ...schema, tables: [{ ...schema.tables[0], columns: [{name:"refreshed",dataType:"TEXT",references:[]}]}] }} />);
  expect(provider.provideCompletionItems(model, {} as never).suggestions.map((s) => s.label)).toEqual(["refreshed"]);
  view.rerender(<CodeEditor {...props} />);
  expect(provider.provideCompletionItems(model, {} as never).suggestions).toEqual([]);
  expect(invoke.mock.calls.filter(([command]) => command !== "ui_ready")).toEqual([]);
  view.unmount();
  expect(completionProvider.dispose).toHaveBeenCalledOnce();
});


test.each(['"', '[', '`'])("SQL quoted suggestions survive Monaco's real filter for %s", (opener) => {
  const closer = opener === '[' ? ']' : opener;
  const schema = {
    kind: opener === '[' ? "sqlserver" as const : "sqlite" as const,
    tables: [{ schema: "main", name: "users", columns: [{ name: "email", dataType: "TEXT", references: [] }] }],
  };
  const props = { id: "quoted-sql", language: "sql", source: "", onChange: vi.fn(), onRun: vi.fn() };
  const view = render(<CodeEditor {...props} sqlSchema={schema} />);
  let source = "", offset = 0;
  const model = {
    getValue: () => source,
    getOffsetAt: () => offset,
    getPositionAt: (offset: number) => ({ lineNumber: 1, column: offset + 1 }),
  };
  act(() => mounted.callback?.({
    addCommand: vi.fn(), getModel: () => model,
    getDomNode: () => document.body, getOption: () => false,
  }));
  // Use Monaco's production matcher; the editor shell alone is mocked.
  const score = (pattern: string, word: string) => fuzzyScore(
    pattern, pattern.toLowerCase(), 0, word, word.toLowerCase(), 0,
    { firstMatchCanBeWeak: false, boostFullMatch: true },
  );
  for (const qualifier of ["", "u."]) {
    for (const prefix of ["", "em"]) {
      for (const suffix of [`ail${closer}`, ""]) {
        const typed = opener + prefix;
        source = suffix
          ? `SELECT ${qualifier}${typed}${suffix} FROM main.users u`
          : `SELECT * FROM main.users u WHERE ${qualifier}${typed}`;
        offset = source.indexOf(typed) + typed.length;
        const item = completionProvider.current!.provideCompletionItems(model, {}).suggestions.find((s) => s.label === "email")!;
        expect(item).toBeDefined();
        expect(score(typed, item.label)).toBeUndefined();
        expect(score(typed, item.filterText ?? item.label)).toBeDefined();
        expect(item.insertText).toBe(opener === '[' ? '[email]' : '"email"');
      }
    }
  }
  source = `SELECT * FROM main.${opener}us${closer}`;
  offset = source.length - 1;
  const table = completionProvider.current!.provideCompletionItems(model, {}).suggestions.find((s) => s.label === "users")!;
  expect(table).toBeDefined();
  expect(score(opener + "us", table.filterText ?? table.label)).toBeDefined();
  view.unmount();
});
