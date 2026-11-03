import { render, waitFor } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import CodeEditor from "./CodeEditor";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke,
  Channel: class { onmessage = () => {}; },
}));
vi.mock("@monaco-editor/react", () => ({
  loader: { config: vi.fn() },
  default: () => <div />,
}));
vi.mock("monaco-editor/editor/editor.api.js", () => ({
  editor: { setModelMarkers: vi.fn() },
  MarkerSeverity: { Error: 8 },
}));
vi.mock("monaco-editor/languages/definitions/python/register.js", () => ({}));
vi.mock("monaco-editor/languages/definitions/csharp/register.js", () => ({}));
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
