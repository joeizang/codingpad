import { afterEach, beforeAll, expect, test } from "vitest";
import type * as Monaco from "monaco-editor/editor/editor.api.js";

// These tests use the real Monaco, not the mock the component tests use, so a
// missing editor contribution fails here instead of only in the app.
let monaco: typeof Monaco;
let editor: Monaco.editor.IStandaloneCodeEditor | undefined;

beforeAll(async () => {
  window.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
  })) as unknown as typeof window.matchMedia;
  monaco = await import("monaco-editor/editor/editor.api.js");
  await import("./monacoFeatures");
});

afterEach(() => {
  editor?.dispose();
  editor = undefined;
  document.body.replaceChildren();
});

function create(value: string) {
  const host = document.createElement("div");
  document.body.append(host);
  editor = monaco.editor.create(host, { value });
  return editor;
}

test("the editor has the completion widget, hover and find", () => {
  const instance = create("print(1)");
  for (const id of [
    "editor.contrib.suggestController",
    "editor.contrib.contentHover",
    "editor.contrib.findController",
  ]) {
    expect(instance.getContribution(id), id).not.toBeNull();
  }
});

test("word commands edit Monaco's text instead of the hidden textarea", () => {
  const instance = create("first second third");
  instance.setPosition({ lineNumber: 1, column: 19 });
  instance.trigger("keyboard", "cursorWordLeft", null);
  expect(instance.getPosition()?.column).toBe(14);
  instance.trigger("keyboard", "deleteWordLeft", null);
  expect(instance.getValue()).toBe("first third");
  instance.trigger("keyboard", "deleteWordRight", null);
  expect(instance.getValue()).toBe("first ");
});

test("long lines wrap at the editor's width instead of scrolling sideways", async () => {
  const { EDITOR_OPTIONS } = await import("./editorOptions");
  const host = document.createElement("div");
  document.body.append(host);
  editor = monaco.editor.create(host, { ...EDITOR_OPTIONS, automaticLayout: false, value: "x = 1" });
  const wrapping = editor.getOption(monaco.editor.EditorOption.wrappingInfo);
  expect(wrapping.isViewportWrapping).toBe(true);
  expect(editor.getOption(monaco.editor.EditorOption.wrappingIndent))
    .toBe(monaco.editor.WrappingIndent.Indent);
});
