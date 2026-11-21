import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type RefObject,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { Connection } from "./ConnectionsDialog";
import { baseName } from "./library";

import { SCRIPT_LANGUAGES, scriptLanguageDetails, type ScriptLanguage } from "./scriptLanguages";

/** The bundled database every install has; it is not in the saved connection list. */
export const SAMPLE_CONNECTION = "codingpad-sample";

type Choice = { name: string; detail: string };

type Props = {
  open: boolean;
  anchor: RefObject<HTMLElement | null>;
  /** The connection picked last time, preselected when SQL needs one. */
  lastConnection?: string;
  onCreate: (language: ScriptLanguage, connection?: string) => void;
  onClose: () => void;
};

/** A database file becomes a connection named after it, unique among `taken`. */
export function connectionNameForFile(path: string, taken: string[]): string {
  const stem = baseName(path).replace(/\.[^.]*$/, "");
  const base = stem.replace(/[^A-Za-z0-9_-]+/g, "-").replace(/^-+|-+$/g, "") || "database";
  let name = base;
  for (let n = 2; taken.includes(name) || name === SAMPLE_CONNECTION; n++) name = `${base}-${n}`;
  return name;
}

export function NewScriptMenu({ open, anchor, lastConnection, onCreate, onClose }: Props) {
  const panel = useRef<HTMLDivElement>(null);
  const [language, setLanguage] = useState<ScriptLanguage>("python");
  const [connection, setConnection] = useState<string | undefined>();
  const [saved, setSaved] = useState<Connection[]>([]);
  const [error, setError] = useState<string | undefined>();
  const [position, setPosition] = useState({ left: 0, top: 0 });

  const choices: Choice[] = [
    ...saved.map((item) => ({ name: item.name, detail: item.kind })),
    { name: SAMPLE_CONNECTION, detail: "sample" },
  ];
  const fallback =
    choices.find((choice) => choice.name === lastConnection)?.name ?? choices[0].name;
  const current = scriptLanguageDetails(language)!;
  const requiresConnection = current.connection === "required";
  const picked = requiresConnection ? connection ?? fallback : connection;

  async function loadConnections() {
    try {
      const loaded = await invoke<Connection[]>("list_connections");
      setSaved(Array.isArray(loaded) ? loaded : []);
    } catch (failure) {
      setError(String(failure));
    }
  }

  useEffect(() => {
    if (!open) return;
    setLanguage("python");
    setConnection(undefined);
    setError(undefined);
    void loadConnections();
  }, [open]);

  useLayoutEffect(() => {
    if (!open) return;
    const rect = anchor.current?.getBoundingClientRect();
    if (rect) {
      const width = panel.current?.offsetWidth ?? 440;
      setPosition({
        left: Math.max(8, Math.min(rect.left, window.innerWidth - width - 8)),
        top: rect.bottom,
      });
    }
    panel.current?.querySelector<HTMLElement>('[role="radio"][aria-checked="true"]')?.focus();
  }, [open, anchor]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (panel.current?.contains(target) || anchor.current?.contains(target)) return;
      onClose();
    };
    document.addEventListener("mousedown", onPointerDown);
    return () => document.removeEventListener("mousedown", onPointerDown);
  }, [open, anchor, onClose]);

  if (!open) return null;

  const create = () => onCreate(language, picked);
  const close = () => {
    onClose();
    anchor.current?.focus();
  };

  function choose(next: ScriptLanguage) {
    setLanguage(next);
    panel.current?.querySelector<HTMLElement>(`[data-language="${next}"]`)?.focus();
  }

  async function openDatabaseFile() {
    const selected = await openDialog({
      multiple: false,
      title: "Open a SQLite database",
      filters: [
        { name: "SQLite", extensions: ["db", "sqlite", "sqlite3"] },
        { name: "All files", extensions: ["*"] },
      ],
    });
    if (typeof selected !== "string") return;
    const existing = saved.find((item) => item.kind === "sqlite" && item.path === selected);
    if (existing) {
      setConnection(existing.name);
      return;
    }
    try {
      const created = await invoke<Connection>("save_connection", {
        connection: {
          id: "",
          name: connectionNameForFile(selected, saved.map((item) => item.name)),
          kind: "sqlite",
          path: selected,
        },
      });
      await loadConnections();
      setConnection(created.name);
      setError(undefined);
    } catch (failure) {
      setError(String(failure));
    }
  }

  function onKeyDown(event: ReactKeyboardEvent) {
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    const key = event.key.toLowerCase();
    const index = SCRIPT_LANGUAGES.findIndex((item) => item.id === language);
    if (key === "escape") {
      event.preventDefault();
      close();
    } else if (key === "enter") {
      // Enter on "Open a database file…" opens the file picker instead.
      if ((event.target as HTMLElement).classList.contains("add-connection")) return;
      event.preventDefault();
      create();
    } else if (key === "arrowdown" || key === "arrowup") {
      event.preventDefault();
      const step = key === "arrowdown" ? 1 : SCRIPT_LANGUAGES.length - 1;
      choose(SCRIPT_LANGUAGES[(index + step) % SCRIPT_LANGUAGES.length].id);
    } else {
      const match = SCRIPT_LANGUAGES.find((item) => item.key.toLowerCase() === key);
      if (match) {
        event.preventDefault();
        choose(match.id);
      }
    }
  }

  return (
    <div
      ref={panel}
      className="new-script"
      role="dialog"
      aria-label="New script"
      style={{ left: position.left, top: position.top }}
      onKeyDown={onKeyDown}
    >
      <div role="radiogroup" aria-label="Language">
        {SCRIPT_LANGUAGES.map((item) => (
          <button
            key={item.id}
            className="new-script-language"
            role="radio"
            aria-label={item.name}
            aria-checked={item.id === language}
            tabIndex={item.id === language ? 0 : -1}
            data-language={item.id}
            onClick={() => choose(item.id)}
          >
            <span className="language-chip">{item.chip}</span>
            <span>
              <strong>{item.name}</strong>
              <small>{item.detail}</small>
            </span>
            <kbd aria-hidden>{item.key}</kbd>
          </button>
        ))}
      </div>
      <div className="new-script-connection" role="group" aria-label="Connection">
        <p>
          {requiresConnection ? (
            <>
              <em>SQL runs against a connection.</em> Pick one.
            </>
          ) : (
            <>
              Attach a connection to query it with <code>{language === "rust" ? "codingpad::db" : "db"}</code>.
            </>
          )}
        </p>
        <div className="connection-choices">
          <button
            aria-pressed={picked === undefined}
            disabled={requiresConnection}
            onClick={() => setConnection(undefined)}
          >
            None
          </button>
          {choices.map((choice) => (
            <button
              key={choice.name}
              aria-pressed={picked === choice.name}
              onClick={() => setConnection(choice.name)}
            >
              {choice.name}
              <span>{choice.detail}</span>
            </button>
          ))}
          <button className="add-connection" onClick={() => void openDatabaseFile()}>
            Open a database file…
          </button>
        </div>
        {error && (
          <p className="new-script-error" role="alert">
            {error}
          </p>
        )}
      </div>
      <div className="new-script-footer">
        <span>
          <kbd>Ctrl N</kbd> opens this. <kbd>Esc</kbd> closes.
        </span>
        <button className="new-script-create" onClick={create}>
          Create {current.name} script{picked ? ` on ${picked}` : ""}
          <kbd aria-hidden>↵</kbd>
        </button>
      </div>
    </div>
  );
}

