import { useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import CodeEditor from "./CodeEditor";
import { DumpTree } from "./DumpTree";
import { appendOutput, type KernelEvent, type Output } from "./protocol";

const SAMPLE = `from codingpad import dump

print("Hello from CodingPad!")

dump({"name": "Ada", "languages": ["Python", "C#", "Rust"]})

# The last expression is displayed automatically.
6 * 7
`;

type Script = {
  id: string;
  name: string;
  source: string;
  output: Output[];
  running: boolean;
  status: string;
  error?: string;
};
const newScript = (number: number): Script => ({
  id: `scratch-${number}`,
  name: `Scratch ${number}.py`,
  source: SAMPLE,
  output: [],
  running: false,
  status: "Ready",
});

export default function App() {
  const [scripts, setScripts] = useState<Script[]>([newScript(1)]);
  const [activeId, setActiveId] = useState("scratch-1");
  const counter = useRef(1);
  const [closing, setClosing] = useState(false);
  const generation = useRef(new Map<string, number>());
  const running = useRef(new Set<string>());
  const active = scripts.find((script) => script.id === activeId)!;
  const update = (id: string, change: (script: Script) => Script) =>
    setScripts((all) =>
      all.map((script) => (script.id === id ? change(script) : script)),
    );

  async function runScript(script: Script) {
    if (running.current.has(script.id)) return;
    running.current.add(script.id);
    const version = (generation.current.get(script.id) ?? 0) + 1;
    generation.current.set(script.id, version);
    update(script.id, (item) => ({
      ...item,
      output: [],
      running: true,
      status: "Running…",
      error: undefined,
    }));
    const onEvent = new Channel<KernelEvent>();
    onEvent.onmessage = (event) => {
      if (generation.current.get(script.id) === version) {
        update(script.id, (item) => ({
          ...item,
          output: appendOutput(item.output, event),
        }));
      }
    };
    try {
      const result = await invoke<{ ok: boolean }>("run_script", {
        scriptId: script.id,
        source: script.source,
        onEvent,
      });
      update(script.id, (item) => ({
        ...item,
        status: result.ok ? "Finished" : "Script error",
      }));
    } catch (error) {
      update(script.id, (item) => ({
        ...item,
        status: "Could not run",
        error: String(error),
      }));
    } finally {
      running.current.delete(script.id);
      update(script.id, (item) => ({ ...item, running: false }));
    }
  }

  function addScript() {
    const script = newScript(++counter.current);
    setScripts((all) => [...all, script]);
    setActiveId(script.id);
  }

  async function closeScript(script: Script) {
    if (script.running || closing || scripts.length === 1) return;
    setClosing(true);
    try {
      await invoke("close_script", { scriptId: script.id });
      setScripts((all) => all.filter((item) => item.id !== script.id));
      setActiveId((current) =>
        current === script.id
          ? scripts.find((item) => item.id !== script.id)!.id
          : current,
      );
    } catch (error) {
      update(script.id, (item) => ({ ...item, error: String(error) }));
    } finally {
      setClosing(false);
    }
  }

  // The editor command reads current state instead of its mount-time closure.
  const runActive = useRef(() => {});
  runActive.current = () => {
    void runScript(active);
  };

  return (
    <main className="app">
      <header className="toolbar">
        <div className="brand">
          <span className="brand-mark">c.</span>
          <strong>CodingPad</strong>
          <span className="language">Python</span>
        </div>
        <button
          className="run-button"
          disabled={active.running}
          onClick={() => void runScript(active)}
        >
          {active.running ? "Running…" : "▶ Run"}
        </button>
      </header>
      <nav className="tabs" aria-label="Scratch scripts">
        {scripts.map((script) => (
          <div
            className={`tab ${script.id === activeId ? "active" : ""}`}
            key={script.id}
          >
            <button
              aria-current={script.id === activeId ? "page" : undefined}
              onClick={() => setActiveId(script.id)}
            >
              {script.name}
              {script.running ? " · running" : ""}
            </button>
            {scripts.length > 1 && (
              <button
                aria-label={`Close ${script.name}`}
                disabled={script.running || closing}
                onClick={() => void closeScript(script)}
              >
                ×
              </button>
            )}
          </div>
        ))}
        <button
          className="add-tab"
          aria-label="New scratch script"
          onClick={addScript}
        >
          +
        </button>
      </nav>
      <div className="workspace">
        <section className="editor-pane" aria-label="Python editor">
          <div className="pane-heading">
            <span>EDITOR</span>
            <span>Ctrl / ⌘ + Enter to run</span>
          </div>
          <div className="editor">
            <CodeEditor
              key={active.id}
              id={active.id}
              source={active.source}
              onChange={(source) =>
                update(active.id, (script) => ({ ...script, source }))
              }
              onRun={() => runActive.current()}
            />
          </div>
        </section>
        <section className="results-pane" aria-label="Results">
          <div className="pane-heading">
            <span>RESULTS</span>
            <span role="status">{active.status}</span>
          </div>
          <div className="results" aria-busy={active.running}>
            {active.error && (
              <div className="error" role="alert">
                {active.error}
              </div>
            )}
            {active.output.length === 0 && !active.error && (
              <div className="empty">
                {active.running
                  ? "Waiting for output…"
                  : "Run your script to see printed output and values here."}
              </div>
            )}
            {active.output.map((output, index) =>
              output.kind === "stream" ? (
                <pre className={`stream ${output.stream}`} key={index}>
                  {output.text}
                </pre>
              ) : (
                <div className="dump" key={index}>
                  <DumpTree node={output.node} />
                </div>
              ),
            )}
          </div>
        </section>
      </div>
      <footer>
        <span>{active.name} · Scratch scripts are unsaved</span>
        <span>Python · Basic dump preview</span>
      </footer>
    </main>
  );
}
