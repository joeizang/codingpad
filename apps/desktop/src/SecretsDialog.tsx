import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type SecretOrigin = "vault" | "session";

type SecretEntry = {
  name: string;
  origin: SecretOrigin;
};

type SecretsDialogProps = {
  open: boolean;
  onClose: () => void;
};

export function SecretsDialog({ open, onClose }: SecretsDialogProps) {
  const [entries, setEntries] = useState<SecretEntry[]>([]);
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [vault, setVault] = useState<boolean | null>(null);
  const [status, setStatus] = useState<string | undefined>();
  const [error, setError] = useState<string | undefined>();
  const [sessionOffer, setSessionOffer] = useState(false);
  const [busy, setBusy] = useState(false);
  const valueInput = useRef<HTMLInputElement>(null);
  const dialog = useRef<HTMLElement>(null);

  useEffect(() => {
    if (!open) return;
    setStatus(undefined);
    setError(undefined);
    setSessionOffer(false);
    setValue("");
    void refresh();
    dialog.current?.focus();
  }, [open]);

  async function refresh(): Promise<SecretEntry[] | undefined> {
    try {
      const [listed, available] = await Promise.all([
        invoke<SecretEntry[]>("list_secrets"),
        invoke<boolean | null>("vault_available"),
      ]);
      setEntries(listed);
      setVault(available);
      return listed;
    } catch (failure) {
      setError(String(failure));
      return undefined;
    }
  }

  function choose(secret: string) {
    setName(secret);
    setValue("");
    setStatus(undefined);
    setError(undefined);
    setSessionOffer(false);
    valueInput.current?.focus();
  }

  async function save(sessionOnly = false) {
    if (busy) return;
    setBusy(true);
    setError(undefined);
    setStatus(undefined);
    setSessionOffer(false);
    try {
      const origin = await invoke<SecretOrigin>("save_secret", {
        name,
        value,
        sessionOnly,
      });
      setStatus(
        origin === "vault"
          ? `Saved "${name.trim()}" to the OS password vault.`
          : `Saved "${name.trim()}" for this session only; it disappears when CodingPad exits.`,
      );
      setVault(origin === "vault" ? true : vault === true ? vault : false);
      setValue("");
      await refresh();
    } catch (failure) {
      const message = String(failure);
      setError(message);
      // Only a vault failure needs the explicit session fallback. A bad name or
      // settings write would fail the same way again.
      setSessionOffer(!sessionOnly && message.includes("OS password vault"));
    } finally {
      setBusy(false);
    }
  }

  async function remove(entry: SecretEntry) {
    if (busy) return;
    setBusy(true);
    setError(undefined);
    setStatus(undefined);
    setSessionOffer(false);
    try {
      await invoke("delete_secret", {
        name: entry.name,
        sessionOnly: entry.origin === "session",
      });
      if (name === entry.name) {
        setName("");
        setValue("");
      }
      const remaining = await refresh();
      if (entry.origin === "session" && remaining?.some((item) => item.name === entry.name)) {
        setStatus(
          `Removed the session value for "${entry.name}"; the vault value is still in use.`,
        );
      } else {
        setStatus(`Removed "${entry.name}".`);
      }
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
        className="modal secrets-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="Secrets"
        tabIndex={-1}
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
        }}
      >
        <header className="modal-heading">
          <h2>Secrets</h2>
          <button className="section-action" aria-label="Close secrets" onClick={onClose}>
            ×
          </button>
        </header>
        <p className="modal-note">
          {vault === false
            ? "The OS password vault is unavailable. You can still store a value for this session only."
            : vault === true
              ? "Secret values never appear in scripts or logs. They are kept in the OS password vault."
              : "Secret values never appear in scripts or logs. They are kept in the OS password vault; if it is unavailable, you can store a value for this session only."}
        </p>
        <div className="secret-list" aria-label="Named secrets">
          {entries.length === 0 ? (
            <p className="tree-status">No secrets yet</p>
          ) : (
            entries.map((entry) => (
              <div
                className={`secret-row ${entry.name === name ? "active" : ""}`}
                key={entry.name}
              >
                <button className="secret-name" onClick={() => choose(entry.name)}>
                  {entry.name}
                  {entry.origin === "session" && (
                    <span className="muted"> · this session</span>
                  )}
                </button>
                <button
                  className="section-action"
                  aria-label={`Remove ${entry.name}`}
                  disabled={busy}
                  onClick={() => void remove(entry)}
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
              value={name}
              placeholder="database-password"
              onChange={(event) => setName(event.target.value)}
            />
          </label>
          <label>
            Value
            <input
              ref={valueInput}
              type="password"
              value={value}
              autoComplete="off"
              onChange={(event) => setValue(event.target.value)}
            />
          </label>
          <div className="modal-actions">
            <button
              className="run-button"
              type="submit"
              disabled={busy || !name.trim() || !value}
            >
              {entries.some((entry) => entry.name === name.trim())
                ? "Update secret"
                : "Add secret"}
            </button>
            {sessionOffer && (
              <button
                className="control-button"
                type="button"
                disabled={busy || !name.trim() || !value}
                onClick={() => void save(true)}
              >
                Store for this session only
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
