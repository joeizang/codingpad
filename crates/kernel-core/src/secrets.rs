//! Named secrets backed by the OS password vault.
//!
//! Only secret names are written next to the app settings; values live in the
//! Keychain, Credential Manager or Secret Service. When no vault is available,
//! values fall back to a session-only memory store so scripts keep working
//! until the app exits.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

const SERVICE: &str = "CodingPad";
const NAMES_FILE: &str = "secrets.json";

/// Where a secret value is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretOrigin {
    Vault,
    Session,
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

/// The app-wide secret store. Cloning is not allowed because one store owns the
/// in-memory fallback; share it with `Arc` instead.
pub struct SecretStore {
    names_path: PathBuf,
    names: Mutex<BTreeSet<String>>,
    session: Mutex<BTreeMap<String, String>>,
    cache: Mutex<BTreeMap<String, String>>,
    vault: Arc<dyn Vault>,
    vault_available: AtomicBool,
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
            vault,
            vault_available: AtomicBool::new(true),
        }
    }

    /// Whether values are currently reaching the OS vault. It becomes false
    /// after a vault failure and stays false for the session.
    pub fn vault_available(&self) -> bool {
        self.vault_available.load(Ordering::Relaxed)
    }

    pub fn list(&self) -> Vec<String> {
        let mut names = self.names.lock().unwrap().clone();
        names.extend(self.session.lock().unwrap().keys().cloned());
        names.into_iter().collect()
    }

    pub fn set(&self, name: &str, value: &str) -> Result<SecretOrigin, String> {
        let name = validate_name(name)?;
        let origin = match self.vault.set(name, value) {
            Ok(()) => {
                self.vault_available.store(true, Ordering::Relaxed);
                self.session.lock().unwrap().remove(name);
                SecretOrigin::Vault
            }
            Err(_) => {
                self.vault_available.store(false, Ordering::Relaxed);
                self.session
                    .lock()
                    .unwrap()
                    .insert(name.to_owned(), value.to_owned());
                SecretOrigin::Session
            }
        };
        self.cache
            .lock()
            .unwrap()
            .insert(name.to_owned(), value.to_owned());
        let mut names = self.names.lock().unwrap();
        names.insert(name.to_owned());
        save_names(&self.names_path, &names)?;
        Ok(origin)
    }

    pub fn get(&self, name: &str) -> Result<String, String> {
        let name = validate_name(name)?;
        if let Some(value) = self.cache.lock().unwrap().get(name).cloned() {
            return Ok(value);
        }
        match self.vault.get(name) {
            Ok(Some(value)) => {
                self.vault_available.store(true, Ordering::Relaxed);
                self.cache
                    .lock()
                    .unwrap()
                    .insert(name.to_owned(), value.clone());
                Ok(value)
            }
            Ok(None) => self.session_get(name),
            Err(_) => {
                self.vault_available.store(false, Ordering::Relaxed);
                self.session_get(name)
            }
        }
    }

    fn session_get(&self, name: &str) -> Result<String, String> {
        self.session
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "Secret \"{name}\" was not found. Add it in Secrets… or set it for this session."
                )
            })
    }

    pub fn delete(&self, name: &str) -> Result<(), String> {
        let name = validate_name(name)?;
        let vault_error = self.vault.delete(name).err();
        let had_session_value = self.session.lock().unwrap().remove(name).is_some();
        self.cache.lock().unwrap().remove(name);
        self.names.lock().unwrap().remove(name);
        save_names(&self.names_path, &self.names.lock().unwrap())?;
        if let Some(error) = vault_error {
            // Removing a session-only value is complete even without a vault. If
            // the value may live in the vault, say its entry could not be removed.
            if !had_session_value {
                return Err(format!(
                    "{error} The secret was removed from this session, but its vault entry could not be deleted."
                ));
            }
        }
        Ok(())
    }

    /// Values handed to kernels before a run. Missing or unreadable entries are
    /// omitted; the script gets the same clear error from `secret()` instead.
    pub fn snapshot(&self) -> BTreeMap<String, String> {
        self.list()
            .into_iter()
            .filter_map(|name| self.get(&name).ok().map(|value| (name, value)))
            .collect()
    }
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
            assert_eq!(store.list(), vec!["token"]);
        }
        let text = fs::read_to_string(directory.path().join(NAMES_FILE)).unwrap();
        assert!(text.contains("token"));
        assert!(!text.contains("s3cret"));
        let store = SecretStore::with_vault(directory.path(), vault);
        assert_eq!(store.get("token").unwrap(), "s3cret");
        store.delete("token").unwrap();
        assert!(store.get("token").is_err());
        assert!(store.list().is_empty());
    }

    #[test]
    fn falls_back_to_the_session_without_a_vault() {
        let (_directory, store) = store(Arc::new(BrokenVault));
        assert_eq!(store.set("token", "value").unwrap(), SecretOrigin::Session);
        assert!(!store.vault_available());
        assert_eq!(store.get("token").unwrap(), "value");
        store.delete("token").unwrap();
        assert!(store.get("token").is_err());
    }

    #[test]
    fn missing_secret_has_an_actionable_error() {
        let (_directory, store) = store(Arc::new(MemoryVault::default()));
        let error = store.get("missing").unwrap_err();
        assert!(error.contains("\"missing\""));
        assert!(error.contains("Secrets"));
    }

    #[test]
    fn snapshot_omits_missing_values() {
        let (_directory, store) = store(Arc::new(MemoryVault::default()));
        store.set("one", "1").unwrap();
        store.set("empty", "").unwrap();
        let snapshot = store.snapshot();
        assert_eq!(snapshot.get("one").map(String::as_str), Some("1"));
        assert_eq!(snapshot.get("empty").map(String::as_str), Some(""));
    }
}
