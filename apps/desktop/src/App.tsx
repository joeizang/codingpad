import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  ask,
  open as openDialog,
  save as saveDialog,
} from "@tauri-apps/plugin-dialog";
import CodeEditor, { type EditorHandle } from "./CodeEditor";
import { ConnectionsDialog, type Connection } from "./ConnectionsDialog";
import { CrashScreen } from "./CrashScreen";
import { usePendingCrash } from "./crash";
import {
  DumpTree,
  type ExportFormat,
  type ExportSummary,
} from "./DumpTree";
import { uniqueId, type DiagnosticsSummary } from "./lsp";
import {
  appendOutputBatch,
  collectDumpUpdates,
  type CompileDiagnostic,
  type DumpNode,
  type KernelEvent,
  type Output,
} from "./protocol";
import {
  NewScriptMenu,
  SAMPLE_CONNECTION,
  type ScriptLanguage,
} from "./NewScriptMenu";
import { SecretsDialog } from "./SecretsDialog";
import { SchemaPanel } from "./SchemaPanel";
import { UpdateNotice } from "./UpdateNotice";
import { useAvailableUpdate } from "./updates";
import { useConnectionSchema } from "./schema";
import { ConnectionList, Sidebar } from "./Sidebar";
import {
  headerSummary,
  joinPath,
  parentPath,
  parseHeader,
  scriptPackages,
  type LibrarySettings,
  type ScriptFile,
} from "./library";
import { languageLabel, type Sample } from "./samples";

const SAMPLE = `from codingpad import dump

print("Hello from CodingPad!")

dump({"name": "Ada", "languages": ["Python", "C#", "Rust"]})

# The last expression is displayed automatically.
6 * 7
`;

const CSHARP_SAMPLE = `Console.WriteLine("Hello from C#!");

new { Name = "Ada", Language = "C#" }.Dump();
await Task.Delay(10);

// A final value-returning call is displayed automatically.
Enumerable.Range(1, 10).Sum();
`;

const TYPESCRIPT_SAMPLE = `import { dump } from "codingpad";

console.log("Hello from TypeScript!");
dump({ name: "Ada", language: "TypeScript" });

// The last expression is displayed automatically.
await Promise.resolve(6 * 7);
`;

const SQL_SAMPLE = `-- codingpad: connection=codingpad-sample

SELECT 1 AS answer;
`;

const SCRIPT_FILTERS = [
  { name: "Python", extensions: ["py"] },
  { name: "C#", extensions: ["cs"] },
  { name: "TypeScript", extensions: ["ts"] },
  { name: "SQL", extensions: ["sql"] },
  { name: "All files", extensions: ["*"] },
];

const scriptLanguage = (script: Script) => {
  const name = script.path ?? script.name;
  if (/\.py$/i.test(name)) return "python";
  if (/\.cs$/i.test(name)) return "csharp";
  if (/\.ts$/i.test(name)) return "typescript";
  if (/\.sql$/i.test(name)) return "sql";
  return "plaintext";
};

type Script = {
  id: string;
  name: string;
  path?: string;
  sample?: string;
  source: string;
  dirty: boolean;
  output: Output[];
  running: boolean;
  status: string;
  version: number;
  updates: Record<string, DumpNode>;
  controlling?: boolean;
  error?: string;
  missingToolchain?: "uv" | "dotnet";
  /** How long the last finished run took, measured from the click. */
  elapsed?: number;
  compilation?: { source: string; diagnostics: CompileDiagnostic[] };
};
function intelligenceLabel(summary: DiagnosticsSummary): string {
  if (summary.message) return summary.message;
  const parts: string[] = [];
  if (summary.errors)
    parts.push(`${summary.errors} error${summary.errors === 1 ? "" : "s"}`);
  if (summary.warnings)
    parts.push(
      `${summary.warnings} warning${summary.warnings === 1 ? "" : "s"}`,
    );
  return parts.length ? parts.join(" · ") : "No type problems";
}

const HEADER_COMMENT: Record<ScriptLanguage, string> = {
  python: "#",
  csharp: "//",
  typescript: "//",
  sql: "--",
};

const SCRIPT_STARTERS: Record<ScriptLanguage, { extension: string; source: string }> = {
  python: { extension: "py", source: SAMPLE },
  csharp: { extension: "cs", source: CSHARP_SAMPLE },
  typescript: { extension: "ts", source: TYPESCRIPT_SAMPLE },
  sql: { extension: "sql", source: SQL_SAMPLE },
};

/** Starter text for a new tab, with a header line when a connection is attached. */
function starterSource(language: ScriptLanguage, connection?: string): string {
  const starter = SCRIPT_STARTERS[language].source;
  if (!connection) return starter;
  const header = `${HEADER_COMMENT[language]} codingpad: connection=${connection}`;
  // The SQL starter already carries a header for the sample database; replace it.
  return parseHeader(starter)
    ? starter.replace(/^[^\n]*/, header)
    : `${header}\n\n${starter}`;
}

const newScript = (
  number: number,
  language: ScriptLanguage = "python",
  connection?: string,
): Script => ({
  id: uniqueId("scratch"),
  name: `Scratch ${number}.${SCRIPT_STARTERS[language].extension}`,
  source: starterSource(language, connection),
  dirty: false,
  output: [],
  running: false,
  status: "Ready",
  version: 0,
  updates: {},
});

const EMPTY_DIAGNOSTICS: CompileDiagnostic[] = [];

const SPLIT_KEY = "codingpad.editorSplit";
const LAST_CONNECTION_KEY = "codingpad.lastConnection";
const clampSplit = (value: number) => Math.min(0.85, Math.max(0.2, value));

// Browser storage can be unavailable; these are conveniences, never required.
function readStored(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}
function writeStored(key: string, value: string) {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // Keep working without remembering.
  }
}

function formatElapsed(ms: number): string {
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)} s`;
}

function plural(count: number, word: string): string {
  return `${count} ${word}${count === 1 ? "" : "s"}`;
}

export default function App() {
  const [scripts, setScripts] = useState<Script[]>(() => [newScript(1)]);
  const scriptsRef = useRef(scripts);
  scriptsRef.current = scripts;
  const [activeId, setActiveId] = useState(() => scripts[0].id);
  const [intelligence, setIntelligence] = useState<DiagnosticsSummary>({
    errors: 0,
    warnings: 0,
  });
  const [settings, setSettings] = useState<LibrarySettings | null>(null);
  const [samples, setSamples] = useState<Sample[]>([]);
  const [samplesError, setSamplesError] = useState<string | undefined>();
  const [libraryError, setLibraryError] = useState<string | undefined>();
  const [refresh, setRefresh] = useState(0);
  const counter = useRef(1);
  const opening = useRef(new Set<string>());
  const [closing, setClosing] = useState(false);
  const [secretsOpen, setSecretsOpen] = useState(false);
  const [connectionsOpen, setConnectionsOpen] = useState(false);
  const [pendingCrash, dismissPendingCrash] = usePendingCrash();
  const [connections, setConnections] = useState<Connection[]>([]);
  const [connectionsRevision, setConnectionsRevision] = useState(0);
  const [newScriptOpen, setNewScriptOpen] = useState(false);
  const newScriptButton = useRef<HTMLButtonElement>(null);
  const [lastConnection, setLastConnection] = useState(
    () => readStored(LAST_CONNECTION_KEY) ?? undefined,
  );
  const [cursor, setCursor] = useState({ line: 1, column: 1 });
  const [split, setSplit] = useState(() => {
    const stored = Number(readStored(SPLIT_KEY));
    return stored ? clampSplit(stored) : 0.58;
  });
  const workspace = useRef<HTMLDivElement>(null);
  const [installProgress, setInstallProgress] = useState<string | null>(null);
  const installingToolchain = useRef(false);
  const [toolchainVersion, setToolchainVersion] = useState(0);
  const generation = useRef(new Map<string, number>());
  const running = useRef(new Set<string>());
  const editor = useRef<EditorHandle | null>(null);
  const frames = useRef(new Map<string, number>());
  const cancelFrame = (id: string) => {
    const frame = frames.current.get(id);
    if (frame !== undefined) cancelAnimationFrame(frame);
    frames.current.delete(id);
  };
  useEffect(() => {
    invoke<LibrarySettings | null>("load_library_settings")
      .then((loaded) => {
        if (
          loaded &&
          typeof loaded.libraryPath === "string" &&
          Array.isArray(loaded.pinnedFolders)
        ) {
          setSettings(loaded);
        }
      })
      .catch((error) => setLibraryError(String(error)));
  }, []);
  useEffect(() => {
    let cancelled = false;
    invoke<Sample[]>("list_samples")
      .then((loaded) => {
        if (!cancelled && Array.isArray(loaded)) {
          setSamples(loaded);
          setSamplesError(undefined);
        }
      })
      .catch((error) => {
        if (!cancelled) setSamplesError(String(error));
      });
    return () => {
      cancelled = true;
    };
  }, []);
  useEffect(() => {
    let cancelled = false;
    invoke<Connection[]>("list_connections")
      .then((loaded) => {
        if (!cancelled && Array.isArray(loaded)) setConnections(loaded);
      })
      .catch(() => {
        // The sidebar list is a convenience; the Connections dialog reports errors.
      });
    return () => {
      cancelled = true;
    };
  }, [connectionsRevision]);
  useEffect(
    () => () => {
      for (const frame of frames.current.values()) cancelAnimationFrame(frame);
      frames.current.clear();
      generation.current.clear();
      running.current.clear();
    },
    [],
  );
  const active = scripts.find((script) => script.id === activeId)!;
  const language = scriptLanguage(active);
  const runnable = language !== "plaintext";
  const availableUpdate = useAvailableUpdate();
  const compileDiagnostics = active.compilation?.source === active.source
    ? active.compilation.diagnostics
    : EMPTY_DIAGNOSTICS;
  const activeHeader = parseHeader(active.source);
  const connectionName =
    language === "typescript" ? undefined : activeHeader?.settings.connection || undefined;
  const [schemaRevision, setSchemaRevision] = useState(0);
  const schemaState = useConnectionSchema(connectionName, schemaRevision);
  const update = (id: string, change: (script: Script) => Script) =>
    setScripts((all) =>
      all.map((script) => (script.id === id ? change(script) : script)),
    );

  async function runScript(script: Script) {
    if (
      installingToolchain.current ||
      running.current.has(script.id) ||
      script.controlling
    )
      return;
    if (scriptLanguage(script) === "plaintext") {
      update(script.id, (item) => ({
        ...item,
        status: "Not runnable",
        error: "Only Python, C# and SQL scripts run in this build.",
      }));
      return;
    }
    if (scriptLanguage(script) === "sql") {
      await runSqlScript(script);
      return;
    }
    running.current.add(script.id);
    const version = (generation.current.get(script.id) ?? 0) + 1;
    generation.current.set(script.id, version);
    cancelFrame(script.id);
    const started = performance.now();
    update(script.id, (item) => ({
      ...item,
      output: [],
      running: true,
      status: "Running…",
      missingToolchain: undefined,
      version,
      updates: {},
      compilation: undefined,
      error: undefined,
      elapsed: undefined,
    }));
    const pending: KernelEvent[] = [];
    const flush = () => {
      cancelFrame(script.id);
      const batch = pending.splice(0);
      if (generation.current.get(script.id) === version && batch.length) {
        const updates = collectDumpUpdates(batch);
        update(script.id, (item) => ({
          ...item,
          output: appendOutputBatch(item.output, batch),
          updates: Object.keys(updates).length
            ? { ...item.updates, ...updates }
            : item.updates,
        }));
      }
    };
    const onEvent = new Channel<KernelEvent>();
    onEvent.onmessage = (event) => {
      // A channel can deliver late, even after its invocation resolved. Keep the
      // generation check so an earlier run cannot append to a newer run's output.
      if (generation.current.get(script.id) !== version) return;
      if (event.method === "run_finished") return;
      if (event.method === "diagnostics") {
        update(script.id, (item) => ({
          ...item,
          compilation: { source: script.source, diagnostics: event.params.diagnostics },
        }));
        return;
      }
      pending.push(event);
      if (!frames.current.has(script.id)) {
        frames.current.set(script.id, requestAnimationFrame(flush));
      }
    };
    try {
      const result = await invoke<{
        ok: boolean;
        stopped?: boolean;
        cacheCleared?: boolean;
      }>("run_script", {
        scriptId: script.id,
        source: script.source,
        language: scriptLanguage(script),
        path: script.path ?? null,
        onEvent,
      });
      update(script.id, (item) => ({
        ...item,
        status: result.cacheCleared
          ? "Stopped · cache cleared"
          : result.stopped
            ? "Stopped"
            : result.ok
              ? "Finished"
              : "Script error",
      }));
    } catch (error) {
      const message = String(error);
      update(script.id, (item) => ({
        ...item,
        status: "Could not run",
        error: message,
        missingToolchain: message.startsWith("uv was not found.")
          ? "uv"
          : message.startsWith(".NET was not found.")
            ? "dotnet"
            : undefined,
      }));
    } finally {
      flush();
      running.current.delete(script.id);
      const elapsed = performance.now() - started;
      update(script.id, (item) =>
        item.version === version
          ? { ...item, running: false, elapsed }
          : { ...item, running: false },
      );
    }
  }

  async function runSqlScript(script: Script) {
    running.current.add(script.id);
    const version = (generation.current.get(script.id) ?? 0) + 1;
    generation.current.set(script.id, version);
    const started = performance.now();
    update(script.id, (item) => ({
      ...item,
      output: [],
      running: true,
      status: "Running…",
      missingToolchain: undefined,
      version,
      updates: {},
      compilation: undefined,
      error: undefined,
      elapsed: undefined,
    }));
    try {
      await invoke("begin_sql_run", { scriptId: script.id });
      const result = await invoke<{
        ok: boolean;
        stopped?: boolean;
        tables: DumpNode[];
      }>("run_sql", {
        scriptId: script.id,
        source: script.source,
        path: script.path ?? null,
      });
      if (generation.current.get(script.id) !== version) return;
      update(script.id, (item) => ({
        ...item,
        output: result.tables.map((node, index) => ({
          id: index,
          kind: "dump" as const,
          node,
        })),
        status: result.stopped ? "Stopped" : "Finished",
      }));
    } catch (failure) {
      if (generation.current.get(script.id) !== version) return;
      update(script.id, (item) => ({
        ...item,
        status: "Could not run",
        error: String(failure),
      }));
    } finally {
      running.current.delete(script.id);
      const elapsed = performance.now() - started;
      update(script.id, (item) =>
        item.version === version
          ? { ...item, running: false, elapsed }
          : { ...item, running: false },
      );
    }
  }

  async function installToolchainAndRun(
    script: Script,
    toolchain: "uv" | "dotnet",
  ) {
    if (installingToolchain.current) return;
    installingToolchain.current = true;
    setInstallProgress(
      toolchain === "uv"
        ? "Downloading and verifying uv…"
        : "Downloading and verifying the .NET install script…",
    );
    update(script.id, (item) => ({ ...item, error: undefined }));
    const onProgress = new Channel<string>();
    onProgress.onmessage = (message) => {
      if (installingToolchain.current) setInstallProgress(message);
    };
    let installed = false;
    try {
      await invoke(toolchain === "uv" ? "install_uv" : "install_dotnet", {
        onProgress,
      });
      installed = true;
      setToolchainVersion((version) => version + 1);
    } catch (error) {
      update(script.id, (item) => ({
        ...item,
        error: String(error),
        missingToolchain: toolchain,
      }));
    } finally {
      installingToolchain.current = false;
      setInstallProgress(null);
    }
    const current = scriptsRef.current.find((item) => item.id === script.id);
    if (installed && current) await runScript(current);
  }

  async function controlScript(script: Script, action: "stop" | "reset") {
    if (
      script.controlling ||
      (action === "stop" && !running.current.has(script.id))
    )
      return;
    update(script.id, (item) => ({
      ...item,
      controlling: true,
      error: undefined,
      status: action === "stop" ? "Stopping…" : "Resetting…",
    }));
    try {
      await invoke(`${action}_script`, { scriptId: script.id });
      if (action === "reset") {
        cancelFrame(script.id);
        generation.current.set(
          script.id,
          (generation.current.get(script.id) ?? 0) + 1,
        );
        update(script.id, (item) => ({
          ...item,
          output: [],
          updates: {},
          version: item.version + 1,
          status: "Reset · cache cleared",
        }));
      }
    } catch (error) {
      update(script.id, (item) => ({
        ...item,
        status: "Could not " + action,
        error: String(error),
      }));
    } finally {
      update(script.id, (item) => ({ ...item, controlling: false }));
    }
  }

  async function expandNode(
    handle: string,
    cursor: string | null,
  ): Promise<DumpNode> {
    const result = await invoke<{ node: DumpNode }>(
      language === "sql" ? "expand_sql" : "expand_node",
      {
        scriptId: active.id,
        handle,
        cursor,
        limit: 100,
      },
    );
    return result.node;
  }

  async function exportTable(
    handle: string,
    format: ExportFormat,
    onProgress: (rows: number, total: number | null) => void,
    signal: AbortSignal,
  ): Promise<ExportSummary | null> {
    const scriptId = active.id;
    const sql = language === "sql";
    const stem = active.name.replace(/\.[^.]*$/, "") || "table";
    const selected = await saveDialog({
      title: format === "csv" ? "Export table to CSV" : "Export table to Excel",
      defaultPath: `${stem}.${format}`,
      filters: [
        format === "csv"
          ? { name: "CSV", extensions: ["csv"] }
          : { name: "Excel workbook", extensions: ["xlsx"] },
      ],
    });
    if (typeof selected !== "string" || !selected || signal.aborted) return null;
    const path = selected.toLowerCase().endsWith(`.${format}`)
      ? selected
      : `${selected}.${format}`;
    const exportId = uniqueId("export");
    const channel = new Channel<{ rows: number; total: number | null }>();
    channel.onmessage = (progress) => onProgress(progress.rows, progress.total);
    const cancel = () => void invoke("cancel_export", { exportId });
    signal.addEventListener("abort", cancel);
    try {
      return await invoke<ExportSummary>("export_table_dump", {
        exportId,
        scriptId,
        handle,
        sql,
        format,
        path,
        onProgress: channel,
      });
    } finally {
      signal.removeEventListener("abort", cancel);
    }
  }

  function openLine(line: number) {
    const current = editor.current;
    if (!current) return;
    current.revealLineInCenter(line);
    current.setPosition({ lineNumber: line, column: 1 });
    current.focus();
  }

  function addScript(language: ScriptLanguage, connection?: string) {
    const script = newScript(++counter.current, language, connection);
    setScripts((all) => [...all, script]);
    setActiveId(script.id);
    setNewScriptOpen(false);
    if (connection) {
      setLastConnection(connection);
      writeStored(LAST_CONNECTION_KEY, connection);
    }
  }

  function resizeTo(next: number) {
    const value = clampSplit(next);
    setSplit(value);
    writeStored(SPLIT_KEY, value.toFixed(3));
  }

  function startResize(event: ReactPointerEvent<HTMLDivElement>) {
    const area = workspace.current;
    if (!area) return;
    event.preventDefault();
    const handle = event.currentTarget;
    handle.setPointerCapture?.(event.pointerId);
    const move = (moved: PointerEvent) => {
      const rect = area.getBoundingClientRect();
      if (rect.height > 0) resizeTo((moved.clientY - rect.top) / rect.height);
    };
    const stop = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", stop);
      handle.removeEventListener("pointercancel", stop);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", stop);
    handle.addEventListener("pointercancel", stop);
  }

  async function closeScript(script: Script) {
    if (script.running || script.controlling || closing || scripts.length === 1)
      return;
    if (
      script.dirty &&
      !(await ask(`Discard unsaved changes to ${script.name}?`, {
        title: "Close unsaved script",
        kind: "warning",
      }))
    )
      return;
    setClosing(true);
    try {
      await invoke("close_script", { scriptId: script.id });
      cancelFrame(script.id);
      generation.current.delete(script.id);
      running.current.delete(script.id);
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

  async function openPath(path: string) {
    const existing = scriptsRef.current.find((script) => script.path === path);
    if (existing) {
      setActiveId(existing.id);
      return;
    }
    if (opening.current.has(path)) return;
    opening.current.add(path);
    try {
      const file = await invoke<ScriptFile>("read_script", { path });
      const alreadyOpen = scriptsRef.current.find(
        (script) => script.path === file.path,
      );
      if (alreadyOpen) {
        setActiveId(alreadyOpen.id);
        return;
      }
      const script: Script = {
        id: uniqueId("file"),
        name: file.name,
        path: file.path,
        source: file.source,
        dirty: false,
        output: [],
        running: false,
        status: "Ready",
        version: 0,
        updates: {},
      };
      scriptsRef.current = [...scriptsRef.current, script];
      setScripts((all) =>
        all.some((item) => item.path === file.path) ? all : [...all, script],
      );
      setActiveId(script.id);
      setLibraryError(undefined);
    } catch (error) {
      setLibraryError(String(error));
    } finally {
      opening.current.delete(path);
    }
  }

  async function openFile() {
    const selected = await openDialog({
      multiple: false,
      title: "Open script",
      filters: SCRIPT_FILTERS,
    });
    if (typeof selected === "string") await openPath(selected);
  }

  function openSample(sample: Sample) {
    const existing = scriptsRef.current.find(
      (script) => script.sample === sample.id,
    );
    if (existing) {
      setActiveId(existing.id);
      return;
    }
    const script: Script = {
      id: uniqueId("sample"),
      name: sample.name,
      sample: sample.id,
      source: sample.source,
      dirty: false,
      output: [],
      running: false,
      status: "Ready",
      version: 0,
      updates: {},
    };
    scriptsRef.current = [...scriptsRef.current, script];
    setScripts((all) => [...all, script]);
    setActiveId(script.id);
  }

  async function saveScript(script: Script, saveAs = false) {
    // Capture the text that is actually written: the user may keep typing while
    // the dialog is open or the write is in progress.
    const source = script.source;
    let path = script.path;
    if (!path || saveAs) {
      const directory = script.path
        ? parentPath(script.path)
        : settings?.libraryPath;
      const defaultPath = directory
        ? joinPath(directory, script.name)
        : script.name;
      const selected = await saveDialog({
        title: "Save script",
        defaultPath,
        filters: SCRIPT_FILTERS,
      });
      if (typeof selected !== "string" || !selected) return;
      path = selected;
    }
    const collision = scriptsRef.current.find(
      (item) => item.id !== script.id && item.path === path,
    );
    if (collision?.running || collision?.controlling) {
      update(script.id, (item) => ({
        ...item,
        error: `${collision.name} is open in a running tab. Stop it before saving over it.`,
      }));
      return;
    }
    if (
      collision?.dirty &&
      !(await ask(
        `${collision.name} has unsaved changes. Saving here replaces it on disk and closes that tab.`,
        { title: "Replace open tab", kind: "warning" },
      ))
    )
      return;
    try {
      const saved = await invoke<ScriptFile>("write_script", {
        path,
        source,
      });
      const apply = (item: Script): Script =>
        item.id === script.id
          ? {
              ...item,
              path: saved.path,
              name: saved.name,
              sample: undefined,
              dirty: item.source !== source,
              error: undefined,
            }
          : item;
      const next = (all: Script[]) =>
        all.filter((item) => item.id !== collision?.id).map(apply);
      scriptsRef.current = next(scriptsRef.current);
      setScripts(next);
      if (collision) {
        cancelFrame(collision.id);
        generation.current.delete(collision.id);
        running.current.delete(collision.id);
        void invoke("close_script", { scriptId: collision.id });
      }
      setRefresh((value) => value + 1);
      setLibraryError(undefined);
    } catch (error) {
      update(script.id, (item) => ({ ...item, error: String(error) }));
    }
  }

  async function persistSettings(next: LibrarySettings) {
    try {
      await invoke("save_library_settings", { settings: next });
      setSettings(next);
      setRefresh((value) => value + 1);
      setLibraryError(undefined);
    } catch (error) {
      setLibraryError(String(error));
    }
  }

  async function changeLibrary() {
    const selected = await openDialog({
      directory: true,
      multiple: false,
      title: "Choose library folder",
    });
    if (typeof selected !== "string" || !settings) return;
    await persistSettings({
      ...settings,
      libraryPath: selected,
      pinnedFolders: settings.pinnedFolders.filter(
        (folder) => folder !== selected,
      ),
    });
  }

  async function addPinnedFolder() {
    const selected = await openDialog({
      directory: true,
      multiple: false,
      title: "Pin a folder",
    });
    if (typeof selected !== "string" || !settings) return;
    if (selected === settings.libraryPath) return;
    await persistSettings({
      ...settings,
      pinnedFolders: [...new Set([...settings.pinnedFolders, selected])],
    });
  }

  async function removePinnedFolder(path: string) {
    if (!settings) return;
    await persistSettings({
      ...settings,
      pinnedFolders: settings.pinnedFolders.filter((folder) => folder !== path),
    });
  }

  // Editor commands read current state instead of their mount-time closure.
  const runActive = useRef(() => {});
  runActive.current = () => {
    void runScript(active);
  };
  const saveActive = useRef(() => {});
  saveActive.current = () => {
    void saveScript(active);
  };
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      try {
        const stop = await getCurrentWindow().onCloseRequested(
          async (event) => {
            if (!scriptsRef.current.some((script) => script.dirty)) return;
            const discard = await ask(
              "Some scripts have unsaved changes. Close CodingPad anyway?",
              { title: "Unsaved changes", kind: "warning" },
            );
            if (!discard) event.preventDefault();
          },
        );
        if (cancelled) stop();
        else unlisten = stop;
      } catch {
        // The browser test environment has no Tauri window to hook.
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        saveActive.current();
      }
      if (
        (event.metaKey || event.ctrlKey) &&
        !event.shiftKey &&
        !event.altKey &&
        event.key.toLowerCase() === "n"
      ) {
        event.preventDefault();
        setNewScriptOpen(true);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const packages = language === "python" ? scriptPackages(active.source) : [];
  const otherHeader = activeHeader
    ? headerSummary({
        ...activeHeader,
        settings: Object.fromEntries(
          Object.entries(activeHeader.settings).filter(([key]) => key !== "connection"),
        ),
      })
    : "";
  const attached = connections.find((item) => item.name === connectionName);
  const dumpCount = active.output.filter((output) => output.kind === "dump").length;
  const runState = active.running
    ? "running"
    : active.error || active.status === "Script error"
      ? "failed"
      : "idle";
  const diagnostics = intelligenceLabel(intelligence);
  const runDisabled =
    !runnable || active.running || active.controlling || installProgress !== null;

  return (
    <main className="app">
      {pendingCrash && (
        <CrashScreen details={pendingCrash} onDismiss={dismissPendingCrash} />
      )}
      <header className="topbar">
        <div className="brand">
          <span className="brand-mark" aria-hidden>
            c.
          </span>
          <strong>CodingPad</strong>
        </div>
        <nav className="tabs" aria-label="Open scripts">
          {scripts.map((script) => {
            const scriptLang = scriptLanguage(script);
            return (
              <div
                className={`tab ${script.id === activeId ? "active" : ""}`}
                key={script.id}
              >
                <button
                  className="tab-open"
                  aria-current={script.id === activeId ? "page" : undefined}
                  title={script.path}
                  onClick={() => setActiveId(script.id)}
                >
                  <span className="language-chip" aria-hidden>
                    {{ csharp: "C#", sql: "SQL", python: "PY", typescript: "TS", plaintext: "TXT" }[scriptLang]}
                  </span>
                  {script.name}
                  {script.dirty && (
                    <span className="dirty-dot" aria-hidden />
                  )}
                  {script.running && <span className="tab-running"> · running</span>}
                </button>
                {scripts.length > 1 && (
                  <button
                    className="tab-close"
                    aria-label={`Close ${script.name}`}
                    disabled={script.running || script.controlling || closing}
                    onClick={() => void closeScript(script)}
                  >
                    ×
                  </button>
                )}
              </div>
            );
          })}
          <button
            ref={newScriptButton}
            className={`new-script-button ${newScriptOpen ? "open" : ""}`}
            aria-label="New script"
            aria-haspopup="dialog"
            aria-expanded={newScriptOpen}
            title="New script (Ctrl+N)"
            onClick={() => setNewScriptOpen((open) => !open)}
          >
            +
          </button>
        </nav>
        <div className="run-controls">
          <button
            className="control-button"
            disabled={!active.running || active.controlling}
            onClick={() => void controlScript(active, "stop")}
          >
            Stop
          </button>
          <button
            className="control-button"
            disabled={active.running || active.controlling}
            onClick={() => void controlScript(active, "reset")}
          >
            Reset
          </button>
          <button
            className="run-button"
            disabled={runDisabled}
            title={
              runnable ? undefined : "Only Python, C# and SQL scripts run in this build"
            }
            onClick={() => void runScript(active)}
          >
            {active.running ? "Running…" : "▶ Run"}
            <kbd aria-hidden>Ctrl ↵</kbd>
          </button>
        </div>
      </header>
      <NewScriptMenu
        open={newScriptOpen}
        anchor={newScriptButton}
        lastConnection={lastConnection}
        onCreate={addScript}
        onClose={() => setNewScriptOpen(false)}
      />
      {availableUpdate && <UpdateNotice update={availableUpdate} />}
      <div className="context-strip">
        <div className="context-sidebar" />
        <span className="context-item">
          Language <b>{languageLabel(language)}</b>
        </span>
        <span className="context-item">
          Connection{" "}
          {connectionName ? (
            <>
              <b>{connectionName}</b>
              {(attached || connectionName === SAMPLE_CONNECTION) && (
                <span className="context-detail">
                  {attached ? attached.kind : "sample"}
                </span>
              )}
            </>
          ) : (
            <span className="context-detail">None</span>
          )}
        </span>
        {packages.length > 0 && (
          <span className="context-item" title={packages.join(", ")}>
            Packages <b>{packages.join(", ")}</b>
          </span>
        )}
        {otherHeader && (
          <span className="context-item" title={activeHeader?.line}>
            Header <b>{otherHeader}</b>
          </span>
        )}
        <div className="context-actions">
          <button aria-label="Save" onClick={() => void saveScript(active)}>
            Save
          </button>
          <button aria-label="Save as" onClick={() => void saveScript(active, true)}>
            Save as…
          </button>
        </div>
      </div>
      <div className="main-area">
        <Sidebar
          settings={settings}
          samples={samples}
          samplesError={samplesError}
          activePath={active.path}
          activeSample={active.sample}
          refresh={refresh}
          onOpen={(path) => void openPath(path)}
          onOpenSample={openSample}
          onOpenFile={() => void openFile()}
          onChangeLibrary={() => void changeLibrary()}
          onAddPinned={() => void addPinnedFolder()}
          onRemovePinned={(path) => void removePinnedFolder(path)}
          error={libraryError}
          top={
            connectionName && (
              <SchemaPanel
                key={connectionName}
                name={connectionName}
                {...schemaState}
                onRefresh={() => setSchemaRevision((value) => value + 1)}
              />
            )
          }
          bottom={
            <ConnectionList
              connections={connections}
              attached={connectionName}
              onManage={() => setConnectionsOpen(true)}
              onSecrets={() => setSecretsOpen(true)}
            />
          }
        />
        <div className="workspace" ref={workspace}>
          <section
            className="editor-pane"
            aria-label={`${languageLabel(language)} editor`}
            style={{ flexBasis: `${split * 100}%` }}
          >
            <div className="editor">
              <CodeEditor
                key={`${active.id}:${language}`}
                id={active.id}
                language={language}
                path={active.path}
                compileDiagnostics={compileDiagnostics}
                toolchainVersion={toolchainVersion}
                sqlSchema={schemaState.schema}
                dataRevision={schemaRevision}
                source={active.source}
                onDiagnostics={setIntelligence}
                onCursor={(line, column) => setCursor({ line, column })}
                onChange={(source) =>
                  update(active.id, (script) => ({
                    ...script,
                    source,
                    dirty: true,
                  }))
                }
                onRun={() => runActive.current()}
                onMount={(instance) => {
                  editor.current = instance;
                }}
              />
            </div>
          </section>
          <div
            className="splitter"
            role="separator"
            aria-orientation="horizontal"
            aria-label="Resize editor and results"
            aria-valuemin={20}
            aria-valuemax={85}
            aria-valuenow={Math.round(split * 100)}
            tabIndex={0}
            onPointerDown={startResize}
            onDoubleClick={() => resizeTo(0.58)}
            onKeyDown={(event) => {
              if (event.key === "ArrowUp" || event.key === "ArrowDown") {
                event.preventDefault();
                resizeTo(split + (event.key === "ArrowUp" ? -0.05 : 0.05));
              }
            }}
          />
          <section className="results-pane" aria-label="Results">
            <div className="results-heading">
              <h2>Results</h2>
              <span className={`run-status ${runState}`} role="status">
                {active.status}
              </span>
              {!active.running && (dumpCount > 0 || active.elapsed !== undefined) && (
                <span className="results-meta">
                  {dumpCount > 0 && plural(dumpCount, "value")}
                  {dumpCount > 0 && active.elapsed !== undefined && ", "}
                  {active.elapsed !== undefined && `took ${formatElapsed(active.elapsed)}`}
                </span>
              )}
            </div>
            <div className="results" aria-busy={active.running || installProgress !== null}>
              {installProgress && <p role="status">{installProgress}</p>}
              {active.missingToolchain && (
                <div className="tool-install">
                  <p>
                    {active.missingToolchain === "dotnet"
                      ? "Install the .NET SDK privately for CodingPad to run C# scripts. This needs an internet connection."
                      : "Install uv privately for CodingPad to run Python scripts. This needs an internet connection."}
                  </p>
                  <button
                    className="control-button"
                    disabled={installProgress !== null || active.running}
                    onClick={() =>
                      void installToolchainAndRun(
                        active,
                        active.missingToolchain!,
                      )
                    }
                  >
                    {installProgress
                      ? active.missingToolchain === "dotnet"
                        ? "Installing .NET…"
                        : "Installing uv…"
                      : active.missingToolchain === "dotnet"
                        ? "Install .NET and run"
                        : "Install uv and run"}
                  </button>
                </div>
              )}
              {active.error && (
                <div className="error" role="alert">
                  {active.error}
                </div>
              )}
              {active.output.length === 0 && !active.error && (
                <div className="empty">
                  {active.running
                    ? "Waiting for output…"
                    : "Run your script with Ctrl+Enter to see printed output and values here."}
                </div>
              )}
              {active.output.map((output) =>
                output.kind === "stream" ? (
                  <pre
                    className={`stream ${output.stream}`}
                    key={`${active.id}:${active.version}:${output.id}`}
                  >
                    {output.text}
                  </pre>
                ) : (
                  <div
                    className="dump"
                    key={`${active.id}:${active.version}:${output.id}`}
                  >
                    <DumpTree
                      node={output.node}
                      updates={active.updates}
                      expand={expandNode}
                      exportTable={exportTable}
                      onOpenLine={openLine}
                    />
                  </div>
                ),
              )}
            </div>
          </section>
        </div>
      </div>
      <ConnectionsDialog
        open={connectionsOpen}
        onClose={() => {
          setConnectionsOpen(false);
          setSchemaRevision((value) => value + 1);
          setConnectionsRevision((value) => value + 1);
        }}
      />
      <SecretsDialog open={secretsOpen} onClose={() => setSecretsOpen(false)} />
      <footer className="statusbar">
        <span className={`state ${runState}`}>
          {active.running ? "running" : runState === "failed" ? "failed" : "ready"}
        </span>
        <span>
          Ln {cursor.line}, Col {cursor.column}
        </span>
        {active.path && <span>{active.dirty ? "Unsaved changes" : "Saved"}</span>}
        <span className="save-state" title={active.path}>
          {active.path
            ? active.path
            : active.sample
              ? `${active.name} · Sample copy · Not saved to the library`
              : `${active.name} · Not saved to the library`}
        </span>
        <span className="statusbar-right">
          <span
            className={`intelligence ${
              intelligence.message
                ? ""
                : intelligence.errors
                  ? "error"
                  : intelligence.warnings
                    ? "warning"
                    : ""
            }`}
            role="status"
            aria-live="polite"
            title={diagnostics}
          >
            {diagnostics}
          </span>
          <span>
            <kbd>Ctrl N</kbd> new
          </span>
          <span>
            <kbd>Ctrl S</kbd> save
          </span>
          <span>
            <kbd>Ctrl ↵</kbd> run
          </span>
        </span>
      </footer>
    </main>
  );
}
