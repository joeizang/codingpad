import { StrictMode } from "react";
import { act, render } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import MonacoEditor from "./MonacoEditor";

const fake = vi.hoisted(() => {
  let value = "";
  let changed = () => {};
  const model = {
    getValue: () => value, getFullModelRange: () => ({}), dispose: vi.fn(),
  };
  const editor = {
    getValue: () => value, getModel: () => model,
    executeEdits: vi.fn((_: string, edits: { text: string }[]) => {
      value = edits[0].text;
      changed();
    }),
    pushUndoStop: vi.fn(), dispose: vi.fn(),
    onDidChangeModelContent: (callback: () => void) => {
      changed = callback;
      return { dispose: vi.fn() };
    },
  };
  return {
    editor, model,
    create: vi.fn(() => editor),
    createModel: vi.fn((source: string) => { value = source; return model; }),
    type: (source: string) => { value = source; changed(); },
  };
});
vi.mock("monaco-editor/editor/editor.api.js", () => ({
  editor: { create: fake.create, createModel: fake.createModel },
  Uri: { parse: (uri: string) => uri },
}));

test("mounts synchronously, keeps edits undoable, and calls the current change handler", () => {
  const onMount = vi.fn();
  const onChange = vi.fn();
  const props = { path: "scratch.py", language: "python", theme: "vs-dark", options: {}, onMount, onChange };
  const view = render(<MonacoEditor {...props} value="41" />);
  expect(onMount).toHaveBeenCalledWith(fake.editor);
  expect(fake.create).toHaveBeenCalledWith(expect.any(HTMLDivElement), expect.objectContaining({ theme: "vs-dark" }));
  const latestChange = vi.fn();
  view.rerender(<MonacoEditor {...props} onChange={latestChange} value="42" />);
  expect(fake.editor.executeEdits).toHaveBeenCalled();
  expect(fake.editor.pushUndoStop).toHaveBeenCalled();
  expect(onChange).not.toHaveBeenCalled();
  expect(latestChange).not.toHaveBeenCalled();
  act(() => fake.type("43"));
  expect(latestChange).toHaveBeenCalledWith("43");
  view.unmount();
  expect(fake.editor.dispose).toHaveBeenCalled();
  expect(fake.model.dispose).toHaveBeenCalled();
});

test("disposes both models and editors during StrictMode mount replay", () => {
  fake.editor.dispose.mockClear();
  fake.model.dispose.mockClear();
  const view = render(<StrictMode><MonacoEditor path="strict.py" language="python" theme="vs-dark" options={{}} value="42" onMount={vi.fn()} onChange={vi.fn()} /></StrictMode>);
  expect(fake.model.dispose).toHaveBeenCalledTimes(1);
  view.unmount();
  expect(fake.model.dispose).toHaveBeenCalledTimes(2);
  expect(fake.editor.dispose).toHaveBeenCalledTimes(2);
});
