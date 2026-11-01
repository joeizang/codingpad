import Editor, { loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor/editor/editor.api.js";
import "monaco-editor/languages/definitions/python/register.js";
import EditorWorker from "monaco-editor/editor/editor.worker.js?worker";

// Bundle Monaco and its worker locally; no CDN or runtime network dependency.
self.MonacoEnvironment = { getWorker: () => new EditorWorker() };
loader.config({ monaco });

export type EditorHandle = {
  revealLineInCenter: (line: number) => void;
  setPosition: (position: { lineNumber: number; column: number }) => void;
  focus: () => void;
};

export default function CodeEditor({
  id,
  source,
  onChange,
  onRun,
  onMount,
}: {
  id: string;
  source: string;
  onChange: (source: string) => void;
  onRun: () => void;
  onMount?: (editor: EditorHandle) => void;
}) {
  return (
    <Editor
      path={`${id}.py`}
      language="python"
      theme="vs-dark"
      value={source}
      onChange={(value) => onChange(value ?? "")}
      onMount={(editor) => {
        editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, onRun);
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
        quickSuggestions: false,
        wordBasedSuggestions: "off",
      }}
      loading={<p className="empty">Loading editor…</p>}
    />
  );
}
