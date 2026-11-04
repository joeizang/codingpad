import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type SecretOrigin = "vault" | "session";

type SecretsDialogProps = {
  open: boolean;
  onClose: () => void;
};

export function SecretsDialog({ open, onClose }: SecretsDialogProps) {
  const [names, setNames] = useState<string[]>([]);
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [vault, setVault] = useState(true);
  const [status, setStatus] = useState<string | undefined>();
  const [error, setError] = useState<string | undefined>();
  const [busy, setBusy] = useState(false);
  const valueInput = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setStatus(undefined);
    setError(undefined);
    setValue("");
    void refresh();
  }, [open]);

  async function refresh() {
    try {
      const [listed, available] = await Promise.all([
        invoke<string[]>("list_secrets"),
        invoke<boolean>("vault_available"),
      ]);
      setNames(listed);
      setVault(available);
    } catch (failure) {
      setError(String(failure));
    }
  }

  function choose(secret: string) {
    setName(secret);
    setValue("");
    setStatus(undefined);
    setError(undefined);
    valueInput.current?.focus();
  }

  async function save() {
    if (busy) return;
    setBusy(true);
    setError(undefined);
    setStatus(undefined);
    try {
      const origin = await invoke<SecretOrigin>("save_secret", { name, value });
      setStatus(
        origin === "vault"
          ? `Saved "${name.trim()}" to the OS password vault.`
          : `Saved "${name.trim()}" for this session only; no OS password vault is available.`,
      );
      setVault(origin === "vault");
      setValue("");
      await refresh();
    } catch (failure) {
      setError(String(failure));
    } finally {
      setBusy(false);
    }
  }

  async function remove(secret: string) {
    if (busy) return;
    setBusy(true);
    setError(undefined);
    setStatus(undefined);
    try {
      await invoke("delete_secret", { name: secret });
      if (name === secret) {
        setName("");
        setValue("");
      }
      setStatus(`Removed "${secret}".`);
      await refresh();
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
        className="modal secrets-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="Secrets"
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
          Secret values never appear in scripts or logs. They are kept in{" "}
          {vault ? "the OS password vault" : "memory for this session because no OS password vault is available"}.
        </p>
        <div className="secret-list" aria-label="Named secrets">
          {names.length === 0 ? (
            <p className="tree-status">No secrets yet</p>
          ) : (
            names.map((secret) => (
              <div className={`secret-row ${secret === name ? "active" : ""}`} key={secret}>
                <button className="secret-name" onClick={() => choose(secret)}>
                  {secret}
                </button>
                <button
                  className="section-action"
                  aria-label={`Remove ${secret}`}
                  disabled={busy}
                  onClick={() => void remove(secret)}
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
              {names.includes(name.trim()) ? "Update secret" : "Add secret"}
            </button>
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
