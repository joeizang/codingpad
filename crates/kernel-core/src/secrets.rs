//! Named secrets backed by the OS password vault.
//!
//! Only secret names are written next to the app settings; values live in the
//! Keychain, Credential Manager or Secret Service. A value can also be stored
//! for the current session only, explicitly, when no vault is available. Session
//! names are not persisted.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex,
    },
};

const SERVICE: &str = "CodingPad";
const NAMES_FILE: &str = "secrets.json";
const UNKNOWN: u8 = 0;
const AVAILABLE: u8 = 1;
const FAILED: u8 = 2;

/// Where a secret value is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretOrigin {
    Vault,
    Session,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretEntry {
    pub name: String,
    pub origin: SecretOrigin,
}

/// Values and read failures for one run. A vault failure is sent to the kernel
/// so `secret()` can report the real reason instead of calling the name missing.
#[derive(Clone, Debug, Default)]
pub struct SecretSnapshot {
    pub values: BTreeMap<String, String>,
    pub errors: BTreeMap<String, String>,
}

/// The platform password vault. Tests inject an in-memory implementation.
pub trait Vault: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<String>, String>;
    fn set(&self, name: &str, value: &str) -> Result<(), String>;
    fn delete(&self, name: &str) -> Result<(), String>;
}

/// Keychain on macOS, Credential Manager on Windows, Secret Service on Linux.
pub struct OsVault;

impl Vault for OsVault {
    fn get(&self, name: &str) -> Result<Option<String>, String> {
        let entry = keyring::Entry::new(SERVICE, name).map_err(vault_error)?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(vault_error(error)),
        }
    }

    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        keyring::Entry::new(SERVICE, name)
            .map_err(vault_error)?
            .set_password(value)
            .map_err(vault_error)
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(SERVICE, name).map_err(vault_error)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(vault_error(error)),
        }
    }
}

fn vault_error(error: keyring::Error) -> String {
    format!("The OS password vault is unavailable: {error}")
}

/// The app-wide secret store. Share it with `Arc`; one store owns the
/// in-memory session values.
pub struct SecretStore {
    names_path: PathBuf,
    names: Mutex<BTreeSet<String>>,
    session: Mutex<BTreeMap<String, String>>,
    cache: Mutex<BTreeMap<String, String>>,
    missing: Mutex<BTreeSet<String>>,
    failures: Mutex<BTreeMap<String, String>>,
    vault: Arc<dyn Vault>,
    vault_state: AtomicU8,
}

impl SecretStore {
    pub fn new(data_dir: &Path) -> Self {
        Self::with_vault(data_dir, Arc::new(OsVault))
    }

    pub fn with_vault(data_dir: &Path, vault: Arc<dyn Vault>) -> Self {
        let names_path = data_dir.join(NAMES_FILE);
        let names = load_names(&names_path);
        Self {
            names_path,
            names: Mutex::new(names),
            session: Mutex::new(BTreeMap::new()),
            cache: Mutex::new(BTreeMap::new()),
            missing: Mutex::new(BTreeSet::new()),
            failures: Mutex::new(BTreeMap::new()),
            vault,
            vault_state: AtomicU8::new(UNKNOWN),
        }
    }

    /// `None` until the vault has been exercised this session. `Some(false)`
    /// means the last vault call failed; values then require an explicit
    /// session-only save.
    pub fn vault_available(&self) -> Option<bool> {
        match self.vault_state.load(Ordering::Relaxed) {
            AVAILABLE => Some(true),
            FAILED => Some(false),
            _ => None,
        }
    }

    fn set_state(&self, state: u8) {
        self.vault_state.store(state, Ordering::Relaxed);
    }

    /// Forget cached vault read failures so opening the Secrets dialog retries
    /// a vault that was locked or denied earlier in the session.
    pub fn clear_failures(&self) {
        self.failures.lock().unwrap().clear();
    }

    pub fn entries(&self) -> Vec<SecretEntry> {
        let names = self.names.lock().unwrap().clone();
        let session = self.session.lock().unwrap();
        let mut all: BTreeSet<String> = names.clone();
        all.extend(session.keys().cloned());
        all.into_iter()
            .map(|name| SecretEntry {
                origin: if session.contains_key(&name) {
                    SecretOrigin::Session
                } else {
                    SecretOrigin::Vault
                },
                name,
            })
            .collect()
    }

    pub fn set(&self, name: &str, value: &str) -> Result<SecretOrigin, String> {
        let name = validate_name(name)?;
        self.vault.set(name, value).map_err(|error| {
            self.set_state(FAILED);
            format!(
                "Could not save secret \"{name}\" to the OS password vault: {error}. Nothing was changed; you can try again or store it for this session only."
            )
        })?;
        self.set_state(AVAILABLE);
        self.session.lock().unwrap().remove(name);
        self.cache
            .lock()
            .unwrap()
            .insert(name.to_owned(), value.to_owned());
        self.missing.lock().unwrap().remove(name);
        self.failures.lock().unwrap().remove(name);
        let mut names = self.names.lock().unwrap();
        names.insert(name.to_owned());
        save_names(&self.names_path, &names)?;
        Ok(SecretOrigin::Vault)
    }

    /// Keep a value in memory for the rest of the session. The name is not
    /// written to `secrets.json`, so it disappears on restart.
    pub fn set_session(&self, name: &str, value: &str) -> Result<(), String> {
        let name = validate_name(name)?;
        self.session
            .lock()
            .unwrap()
            .insert(name.to_owned(), value.to_owned());
        self.cache
            .lock()
            .unwrap()
            .insert(name.to_owned(), value.to_owned());
        self.missing.lock().unwrap().remove(name);
        self.failures.lock().unwrap().remove(name);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<String, String> {
        let name = validate_name(name)?;
        match self.resolve(name)? {
            Some(value) => Ok(value),
            None => Err(missing_secret(name)),
        }
    }

    /// Named credentials for database connections are reserved for the host.
    pub(crate) fn get_for_script(&self, name: &str) -> Result<String, String> {
        let name = validate_name(name)?;
        if is_connection_secret(name) {
            return Err(missing_secret(name));
        }
        self.get(name)
    }

    /// `Ok(None)` means the vault has no entry; `Err` means the vault refused or
    /// failed. Callers report those differently.
    fn resolve(&self, name: &str) -> Result<Option<String>, String> {
        if let Some(value) = self.cache.lock().unwrap().get(name).cloned() {
            return Ok(Some(value));
        }
        if let Some(value) = self.session.lock().unwrap().get(name).cloned() {
            return Ok(Some(value));
        }
        if self.missing.lock().unwrap().contains(name) {
            return Ok(None);
        }
        if let Some(error) = self.failures.lock().unwrap().get(name).cloned() {
            return Err(error);
        }
        match self.vault.get(name) {
            Ok(Some(value)) => {
                self.set_state(AVAILABLE);
                self.cache
                    .lock()
                    .unwrap()
                    .insert(name.to_owned(), value.clone());
                Ok(Some(value))
            }
            Ok(None) => {
                self.set_state(AVAILABLE);
                self.missing.lock().unwrap().insert(name.to_owned());
                Ok(None)
            }
            Err(error) => {
                self.set_state(FAILED);
                let message =
                    format!("Could not read secret \"{name}\" from the OS password vault: {error}");
                self.failures
                    .lock()
                    .unwrap()
                    .insert(name.to_owned(), message.clone());
                Err(message)
            }
        }
    }

    /// Delete a vault-backed secret. `session_only` removes only the in-memory
    /// override, leaving any vault entry untouched.
    pub fn delete(&self, name: &str, session_only: bool) -> Result<(), String> {
        let name = validate_name(name)?;
        if session_only {
            self.session.lock().unwrap().remove(name);
            self.cache.lock().unwrap().remove(name);
            self.missing.lock().unwrap().remove(name);
            self.failures.lock().unwrap().remove(name);
            return Ok(());
        }
        self.vault.delete(name).map_err(|error| {
            self.set_state(FAILED);
            format!(
                "Could not delete secret \"{name}\" from the OS password vault: {error}. The name was kept so you can try again."
            )
        })?;
        self.set_state(AVAILABLE);
        self.session.lock().unwrap().remove(name);
        self.cache.lock().unwrap().remove(name);
        self.missing.lock().unwrap().remove(name);
        self.failures.lock().unwrap().remove(name);
        let mut names = self.names.lock().unwrap();
        names.remove(name);
        save_names(&self.names_path, &names)
    }

    /// Values handed to kernels before a run. Missing or unreadable entries are
    /// omitted; the script gets the same clear error from `secret()` instead.
    /// Database connection passwords (`connection:`) stay in the Rust host and
    /// are never sent to script kernels.
    pub fn snapshot(&self) -> SecretSnapshot {
        self.snapshot_named(None)
    }

    /// Read only `names` when the script names them, or every stored name when
    /// `names` is `None` (a dynamic call). Connection passwords never go to a
    /// script kernel.
    pub fn snapshot_named(&self, names: Option<&[String]>) -> SecretSnapshot {
        let mut snapshot = SecretSnapshot::default();
        let entries: Vec<SecretEntry> = match names {
            Some(names) => names
                .iter()
                .filter(|name| !is_connection_secret(name))
                .map(|name| SecretEntry {
                    name: name.clone(),
                    origin: SecretOrigin::Vault,
                })
                .collect(),
            None => self
                .entries()
                .into_iter()
                .filter(|entry| !is_connection_secret(&entry.name))
                .collect(),
        };
        for entry in entries {
            match self.resolve(&entry.name) {
                Ok(Some(value)) => {
                    snapshot.values.insert(entry.name, value);
                }
                Ok(None) => {}
                Err(error) => {
                    snapshot.errors.insert(entry.name, error);
                }
            }
        }
        snapshot
    }
}

fn is_connection_secret(name: &str) -> bool {
    name.starts_with("connection:")
}

fn missing_secret(name: &str) -> String {
    format!("Secret \"{name}\" was not found. Add it in Secrets… or set it for this session.")
}

fn validate_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A secret name cannot be empty".into());
    }
    if name.chars().any(char::is_control) {
        return Err("A secret name cannot contain control characters".into());
    }
    Ok(name)
}

fn load_names(path: &Path) -> BTreeSet<String> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn save_names(path: &Path, names: &BTreeSet<String>) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create the settings folder: {error}"))?;
    let text = serde_json::to_string_pretty(&names.iter().collect::<Vec<_>>())
        .map_err(|error| format!("Could not encode secret names: {error}"))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("Could not create a temporary file: {error}"))?;
    temp.write_all(text.as_bytes())
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
    temp.persist(path)
        .map_err(|error| format!("Could not save {}: {}", path.display(), error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MemoryVault(Mutex<BTreeMap<String, String>>);
    impl Vault for MemoryVault {
        fn get(&self, name: &str) -> Result<Option<String>, String> {
            Ok(self.0.lock().unwrap().get(name).cloned())
        }
        fn set(&self, name: &str, value: &str) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .insert(name.to_owned(), value.to_owned());
            Ok(())
        }
        fn delete(&self, name: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(name);
            Ok(())
        }
    }

    struct BrokenVault;
    impl Vault for BrokenVault {
        fn get(&self, _: &str) -> Result<Option<String>, String> {
            Err("no vault".into())
        }
        fn set(&self, _: &str, _: &str) -> Result<(), String> {
            Err("no vault".into())
        }
        fn delete(&self, _: &str) -> Result<(), String> {
            Err("no vault".into())
        }
    }

    fn store(vault: Arc<dyn Vault>) -> (tempfile::TempDir, SecretStore) {
        let directory = tempfile::tempdir().unwrap();
        let store = SecretStore::with_vault(directory.path(), vault);
        (directory, store)
    }

    #[test]
    fn persists_names_but_never_values() {
        let directory = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        {
            let store = SecretStore::with_vault(directory.path(), vault.clone());
            assert_eq!(store.set("token", "s3cret").unwrap(), SecretOrigin::Vault);
            assert_eq!(store.get("token").unwrap(), "s3cret");
            assert_eq!(store.entries()[0].name, "token");
            assert_eq!(store.entries()[0].origin, SecretOrigin::Vault);
        }
        let text = fs::read_to_string(directory.path().join(NAMES_FILE)).unwrap();
        assert!(text.contains("token"));
        assert!(!text.contains("s3cret"));
        let store = SecretStore::with_vault(directory.path(), vault);
        assert_eq!(store.get("token").unwrap(), "s3cret");
        store.delete("token", false).unwrap();
        assert!(store.get("token").is_err());
        assert!(store.entries().is_empty());
    }

    #[test]
    fn a_failing_vault_does_not_replace_a_value_silently() {
        let (_directory, store) = store(Arc::new(BrokenVault));
        assert_eq!(store.vault_available(), None);
        let error = store.set("token", "value").unwrap_err();
        assert!(error.contains("Nothing was changed"));
        assert!(store.get("token").is_err());
        assert!(!store.vault_available().unwrap());
    }

    #[test]
    fn session_values_are_explicit_and_never_persisted() {
        let directory = tempfile::tempdir().unwrap();
        let store = SecretStore::with_vault(directory.path(), Arc::new(BrokenVault));
        store.set_session("token", "value").unwrap();
        assert_eq!(store.get("token").unwrap(), "value");
        assert_eq!(
            store.entries(),
            vec![SecretEntry {
                name: "token".into(),
                origin: SecretOrigin::Session,
            }]
        );
        assert!(!directory.path().join(NAMES_FILE).exists());
        store.delete("token", true).unwrap();
        assert!(store.entries().is_empty());
    }

    #[test]
    fn snapshot_reports_vault_errors_and_caches_them_for_the_session() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(NAMES_FILE), "[\"token\"]").unwrap();
        let calls = Arc::new(Mutex::new(0));
        struct CountingVault(Arc<Mutex<usize>>);
        impl Vault for CountingVault {
            fn get(&self, _: &str) -> Result<Option<String>, String> {
                *self.0.lock().unwrap() += 1;
                Err("locked".into())
            }
            fn set(&self, _: &str, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn delete(&self, _: &str) -> Result<(), String> {
                Ok(())
            }
        }
        let store =
            SecretStore::with_vault(directory.path(), Arc::new(CountingVault(calls.clone())));
        let snapshot = store.snapshot();
        assert!(snapshot.values.is_empty());
        assert!(snapshot.errors["token"].contains("locked"));
        assert_eq!(store.get("token").unwrap_err(), snapshot.errors["token"]);
        store.snapshot();
        assert_eq!(
            *calls.lock().unwrap(),
            1,
            "failures are cached for the session"
        );
        store.set("token", "new").unwrap();
        assert_eq!(store.get("token").unwrap(), "new");
    }

    #[test]
    fn missing_secret_has_an_actionable_error() {
        let (_directory, store) = store(Arc::new(MemoryVault::default()));
        let error = store.get("missing").unwrap_err();
        assert!(error.contains("\"missing\""));
        assert!(error.contains("Secrets"));
    }

    #[test]
    fn a_vault_read_error_is_not_reported_as_missing() {
        let (_directory, store) = store(Arc::new(BrokenVault));
        let error = store.get("token").unwrap_err();
        assert!(error.contains("Could not read"));
        assert!(error.contains("no vault"));
        assert!(!error.contains("was not found"));
    }

    #[test]
    fn failed_delete_keeps_the_name_and_reports_the_error() {
        let (_directory, store) = store(Arc::new(BrokenVault));
        store.set_session("token", "value").unwrap();
        let error = store.delete("token", false).unwrap_err();
        assert!(error.contains("Could not delete"));
        assert_eq!(store.get("token").unwrap(), "value");
        store.delete("token", true).unwrap();
        assert!(store.entries().is_empty());
    }

    #[test]
    fn snapshot_named_reads_only_the_requested_secrets() {
        let directory = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        struct RecordingVault {
            values: BTreeMap<String, String>,
            calls: Arc<Mutex<Vec<String>>>,
        }
        impl Vault for RecordingVault {
            fn get(&self, name: &str) -> Result<Option<String>, String> {
                self.calls.lock().unwrap().push(name.to_owned());
                Ok(self.values.get(name).cloned())
            }
            fn set(&self, _: &str, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn delete(&self, _: &str) -> Result<(), String> {
                Ok(())
            }
        }
        let store = SecretStore::with_vault(
            directory.path(),
            Arc::new(RecordingVault {
                values: BTreeMap::from([("one".into(), "1".into()), ("two".into(), "2".into())]),
                calls: calls.clone(),
            }),
        );
        fs::write(directory.path().join(NAMES_FILE), "[\"one\", \"two\"]").unwrap();
        let snapshot = store.snapshot_named(Some(&["one".to_owned()]));
        assert_eq!(snapshot.values.get("one").map(String::as_str), Some("1"));
        assert!(!snapshot.values.contains_key("two"));
        assert_eq!(*calls.lock().unwrap(), vec!["one".to_owned()]);
        store.snapshot_named(Some(&[]));
        assert_eq!(*calls.lock().unwrap(), vec!["one".to_owned()]);
    }

    #[test]
    fn snapshot_never_sends_connection_passwords_to_scripts() {
        let directory = tempfile::tempdir().unwrap();
        let store = SecretStore::with_vault(directory.path(), Arc::new(MemoryVault::default()));
        store.set("api-key", "public").unwrap();
        store.set("connection:pg", "database-password").unwrap();
        let snapshot = store.snapshot();
        assert_eq!(
            snapshot.values.get("api-key").map(String::as_str),
            Some("public")
        );
        assert!(!snapshot.values.contains_key("connection:pg"));
    }

    #[test]
    fn snapshot_omits_missing_values() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(NAMES_FILE),
            "[\"one\", \"empty\", \"absent\"]",
        )
        .unwrap();
        let store = SecretStore::with_vault(directory.path(), Arc::new(MemoryVault::default()));
        store.set("one", "1").unwrap();
        store.set("empty", "").unwrap();
        let snapshot = store.snapshot();
        assert_eq!(snapshot.values.get("one").map(String::as_str), Some("1"));
        assert_eq!(snapshot.values.get("empty").map(String::as_str), Some(""));
        assert!(!snapshot.values.contains_key("absent"));
    }
}
