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
}));
vi.mock("monaco-editor/languages/definitions/python/register.js", () => ({}));
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
