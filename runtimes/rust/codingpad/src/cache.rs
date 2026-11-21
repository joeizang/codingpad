use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, io, path::PathBuf};

/// Reuse the serialized value for this key on later runs of this script.
/// Outside CodingPad, evaluate the factory directly.
pub fn cache<T: Serialize + DeserializeOwned>(key: &str, factory: impl FnOnce() -> T) -> T {
    let Some(root) = std::env::var_os("CODINGPAD_RUST_CACHE") else {
        return factory();
    };
    let root = PathBuf::from(root);
    let entry = root.join(format!("{:x}.json", Sha256::digest(key.as_bytes())));
    match fs::read(&entry) {
        Ok(bytes) => {
            if let Ok(value) = serde_json::from_slice(&bytes) {
                return value;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => panic!("cache() could not read {key:?}: {error}"),
    }

    let value = factory();
    let write = || -> io::Result<()> {
        fs::create_dir_all(&root)?;
        let mut file = tempfile::NamedTempFile::new_in(&root)?;
        serde_json::to_writer(&mut file, &value)?;
        file.persist(entry)?;
        Ok(())
    };
    if let Err(error) = write() {
        panic!("cache() could not store {key:?}: {error}");
    }
    value
}

/// Whether CodingPad has asked this run to stop. Long-running scripts can
/// poll this during their work and return to release resources before exiting.
pub fn is_cancelled() -> bool {
    std::env::var_os("CODINGPAD_RUST_CANCEL").is_some_and(|path| PathBuf::from(path).is_file())
}
