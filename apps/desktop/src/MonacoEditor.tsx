import { useLayoutEffect, useRef } from "react";
import * as monaco from "monaco-editor/editor/editor.api.js";
import "./monacoFeatures";

type Props = {
  path: string;
  language: string;
  value: string;
  theme: string;
  options: monaco.editor.IStandaloneEditorConstructionOptions;
  onChange: (value: string | undefined) => void;
  onMount: (editor: monaco.editor.IStandaloneCodeEditor) => void;
};

/** Monaco is already bundled locally. Mount it in one commit, without the
 * asynchronous loader and extra React commits needed by a CDN-capable wrapper. */
export default function MonacoEditor(props: Props) {
  const container = useRef<HTMLDivElement>(null);
  const instance = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const latest = useRef(props);
  latest.current = props;
  const updating = useRef(false);

  useLayoutEffect(() => {
    performance.mark("editor-create-started");
    const { value, language, path, theme, options } = latest.current;
    const model = monaco.editor.createModel(value, language, monaco.Uri.parse(path));
    const editor = monaco.editor.create(container.current!, { ...options, theme, model });
    instance.current = editor;
    const change = editor.onDidChangeModelContent(() => {
      if (!updating.current) latest.current.onChange(editor.getValue());
    });
    latest.current.onMount(editor);
    // Monaco measures character widths once, at creation. The bundled
    // editor font loads lazily, so that first reading uses the fallback
    // font and the caret drifts from the text it draws. Measure again once
    // the editor font is ready.
    let disposed = false;
    const fontSize = options.fontSize ?? 14;
    document.fonts?.load(`${fontSize}px ${options.fontFamily ?? "monospace"}`)
      .then(() => { if (!disposed) monaco.editor.remeasureFonts(); })
      .catch(() => {});
    return () => {
      disposed = true;
      instance.current = null;
      change.dispose();
      editor.dispose();
      model.dispose();
    };
  }, [props.path, props.language]);

  useLayoutEffect(() => {
    const editor = instance.current;
    const model = editor?.getModel();
    if (!editor || !model || model.getValue() === props.value) return;
    updating.current = true;
    try {
      editor.executeEdits("source-update", [{
        range: model.getFullModelRange(), text: props.value, forceMoveMarkers: true,
      }]);
      editor.pushUndoStop();
    } finally {
      updating.current = false;
    }
  }, [props.value]);

  return <div ref={container} style={{ width: "100%", height: "100%" }} />;
}
