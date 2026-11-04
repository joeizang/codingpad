import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

export type DatabaseKind = "sqlite" | "postgres";

export type Connection = {
  id: string;
  name: string;
  kind: DatabaseKind;
  path?: string | null;
  host?: string | null;
  port?: number | null;
  database?: string | null;
  user?: string | null;
  secret?: string | null;
};

type ConnectionsDialogProps = {
  open: boolean;
  onClose: () => void;
};

const EMPTY: Connection = { id: "", name: "", kind: "sqlite", path: "" };

export function ConnectionsDialog({ open, onClose }: ConnectionsDialogProps) {
  const [connections, setConnections] = useState<Connection[]>([]);
  const [draft, setDraft] = useState<Connection>(EMPTY);
  const [password, setPassword] = useState("");
  const [status, setStatus] = useState<string | undefined>();
  const [error, setError] = useState<string | undefined>();
  const [busy, setBusy] = useState(false);
  const dialog = useRef<HTMLElement>(null);

  useEffect(() => {
    if (!open) return;
    setDraft(EMPTY);
    setPassword("");
    setStatus(undefined);
    setError(undefined);
    void refresh();
    dialog.current?.focus();
  }, [open]);

  async function refresh() {
    try {
      setConnections(await invoke<Connection[]>("list_connections"));
    } catch (failure) {
      setError(String(failure));
    }
  }

  function edit(connection: Connection) {
    setDraft({ ...connection });
    setPassword("");
    setStatus(undefined);
    setError(undefined);
  }

  async function browse() {
    const selected = await openDialog({
      multiple: false,
      title: "Choose a SQLite database",
      filters: [
        { name: "SQLite", extensions: ["db", "sqlite", "sqlite3"] },
        { name: "All files", extensions: ["*"] },
      ],
    });
    if (typeof selected === "string") {
      setDraft((current) => ({ ...current, path: selected }));
    }
  }

  function validate(): string | undefined {
    if (!draft.name.trim()) return "A connection name cannot be empty";
    if (/\s/.test(draft.name.trim())) {
      return "A connection name cannot contain spaces; use letters, digits, dashes or underscores.";
    }
    if (draft.kind === "sqlite" && !draft.path?.trim()) {
      return "A SQLite connection needs a database file";
    }
    return undefined;
  }

  async function save() {
    if (busy) return;
    const invalid = validate();
    if (invalid) {
      setError(invalid);
      return;
    }
    setBusy(true);
    setError(undefined);
    setStatus(undefined);
    try {
      const connection = { ...draft };
      if (connection.kind !== "sqlite" && password) {
        connection.secret = connection.secret || `connection:${connection.name.trim()}`;
        await invoke("save_secret", {
          name: connection.secret,
          value: password,
        });
      }
      const saved = await invoke<Connection>("save_connection", {
        connection,
      });
      setDraft({ ...saved });
      setPassword("");
      setStatus(`Saved "${saved.name}".`);
      await refresh();
    } catch (failure) {
      setError(String(failure));
    } finally {
      setBusy(false);
    }
  }

  async function remove(connection: Connection) {
    if (busy) return;
    setBusy(true);
    setError(undefined);
    setStatus(undefined);
    try {
      await invoke("delete_connection", { id: connection.id });
      if (draft.id === connection.id) setDraft(EMPTY);
      setStatus(`Removed "${connection.name}".`);
      await refresh();
    } catch (failure) {
      setError(String(failure));
    } finally {
      setBusy(false);
    }
  }

  async function test(connection: Connection) {
    if (busy) return;
    setBusy(true);
    setError(undefined);
    setStatus(undefined);
    try {
      setStatus(await invoke<string>("test_connection", { id: connection.id }));
    } catch (failure) {
      setError(String(failure));
    } finally {
      setBusy(false);
    }
  }

  if (!open) return null;
  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <section
        ref={dialog}
        className="modal connections-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="Connections"
        tabIndex={-1}
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
        }}
      >
        <header className="modal-heading">
          <h2>Connections</h2>
          <button className="section-action" aria-label="Close connections" onClick={onClose}>
            ×
          </button>
        </header>
        <p className="modal-note">
          A SQL script names its connection in its first line, for example{" "}
          <code>-- codingpad: connection=sample</code>.
        </p>
        <div className="secret-list" aria-label="Saved connections">
          {connections.length === 0 ? (
            <p className="tree-status">No connections yet</p>
          ) : (
            connections.map((connection) => (
              <div
                className={`secret-row ${connection.id === draft.id ? "active" : ""}`}
                key={connection.id}
              >
                <button className="secret-name" onClick={() => edit(connection)}>
                  {connection.name}
                  <span className="muted"> · {connection.kind}</span>
                </button>
                <button
                  className="section-action"
                  disabled={busy}
                  onClick={() => void test(connection)}
                >
                  Test
                </button>
                <button
                  className="section-action"
                  aria-label={`Remove ${connection.name}`}
                  disabled={busy}
                  onClick={() => void remove(connection)}
                >
                  Remove
                </button>
              </div>
            ))
          )}
        </div>
        <form
          className="secret-form"
          onSubmit={(event) => {
            event.preventDefault();
            void save();
          }}
        >
          <label>
            Name
            <input
              value={draft.name}
              placeholder="northwind"
              onChange={(event) =>
                setDraft((current) => ({ ...current, name: event.target.value }))
              }
            />
          </label>
          <label>
            Kind
            <select
              value={draft.kind}
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  kind: event.target.value as DatabaseKind,
                }))
              }
            >
              <option value="sqlite">SQLite</option>
              <option value="postgres">PostgreSQL</option>
            </select>
          </label>
          {draft.kind === "sqlite" ? (
            <label>
              Database file
              <span className="field-row">
                <input
                  value={draft.path ?? ""}
                  placeholder="/path/to/northwind.db"
                  onChange={(event) =>
                    setDraft((current) => ({ ...current, path: event.target.value }))
                  }
                />
                <button className="control-button" type="button" onClick={() => void browse()}>
                  Browse…
                </button>
              </span>
            </label>
          ) : (
            <>
              <label>
                Host
                <input
                  value={draft.host ?? ""}
                  placeholder="localhost"
                  onChange={(event) =>
                    setDraft((current) => ({ ...current, host: event.target.value }))
                  }
                />
              </label>
              <label>
                Port
                <input
                  type="number"
                  min={1}
                  max={65535}
                  value={draft.port ?? 5432}
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      port: event.target.value ? Number(event.target.value) : null,
                    }))
                  }
                />
              </label>
              <label>
                Database
                <input
                  value={draft.database ?? ""}
                  placeholder="northwind"
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      database: event.target.value,
                    }))
                  }
                />
              </label>
              <label>
                User
                <input
                  value={draft.user ?? ""}
                  placeholder="postgres"
                  onChange={(event) =>
                    setDraft((current) => ({ ...current, user: event.target.value }))
                  }
                />
              </label>
              <label>
                Password
                <input
                  type="password"
                  autoComplete="off"
                  value={password}
                  placeholder={draft.secret ? "Leave blank to keep the saved password" : "Stored in the OS vault"}
                  onChange={(event) => setPassword(event.target.value)}
                />
              </label>
            </>
          )}
          <div className="modal-actions">
            <button
              className="run-button"
              type="submit"
              disabled={busy || !draft.name.trim()}
            >
              {draft.id ? "Save connection" : "Add connection"}
            </button>
            {draft.id && (
              <button
                className="control-button"
                type="button"
                onClick={() => setDraft(EMPTY)}
              >
                New connection
              </button>
            )}
            <button className="control-button" type="button" onClick={onClose}>
              Done
            </button>
          </div>
        </form>
        {status && <p role="status">{status}</p>}
        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
      </section>
    </div>
  );
}
