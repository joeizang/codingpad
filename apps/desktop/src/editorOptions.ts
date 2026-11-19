import type * as monaco from "monaco-editor/editor/editor.api.js";

/** Options for every script editor. */
export const EDITOR_OPTIONS: monaco.editor.IStandaloneEditorConstructionOptions = {
  fontSize: 14,
  fontFamily: "'JetBrains Mono Variable', 'Cascadia Mono', Consolas, 'DejaVu Sans Mono', monospace",
  fontLigatures: false,
  lineHeight: 24,
  renderLineHighlight: "line",
  minimap: { enabled: false },
  automaticLayout: true,
  scrollBeyondLastLine: false,
  padding: { top: 20 },
  tabSize: 4,
  // Wrap long lines at the editor's width instead of scrolling sideways.
  // Continuation rows sit one indent deeper so they read as part of the line
  // above; line numbers still count source lines, so tracebacks match.
  wordWrap: "on",
  wrappingIndent: "indent",
  quickSuggestions: { other: true, comments: false, strings: false },
  wordBasedSuggestions: "off",
  suggestOnTriggerCharacters: true,
};
