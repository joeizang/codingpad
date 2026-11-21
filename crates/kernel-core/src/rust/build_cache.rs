use super::{
    cargo, manifest::Manifest, spawn, CACHE_RUNTIME, RICH_RUNTIME, RUNTIME, RUNTIME_MANIFEST,
};
use crate::shadow::ShadowProject;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, TryLockError},
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tempfile::TempDir;
use tokio::{
    process::Command,
    sync::{watch, OnceCell},
};

const POPULAR_CRATES: &str = r#"---cargo
[dependencies]
anyhow = "1"
regex = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
---
"#;

pub(crate) struct BuildCache {
    persistent: Option<PathBuf>,
    temporary: OnceCell<TempDir>,
    activity: watch::Sender<()>,
    warming: AtomicBool,
}

impl BuildCache {
    pub(crate) fn new(data: Option<&Path>) -> Self {
        Self {
            persistent: data.map(|path| path.join("rust-build")),
            temporary: OnceCell::new(),
            activity: watch::channel(()).0,
            warming: AtomicBool::new(false),
        }
    }

    async fn root(&self) -> Result<&Path, String> {
        if let Some(root) = &self.persistent {
            return Ok(root);
        }
        let temp = self
            .temporary
            .get_or_try_init(|| async { tempfile::tempdir().map_err(cache_error) })
            .await?;
        Ok(temp.path())
    }

    pub(super) async fn acquire(&self) -> Result<BuildLease, String> {
        // An interactive build or Clear can cancel an in-progress warmup.
        self.activity.send_replace(());
        let root = self.root().await?;
        let lock = open_lock(root).map_err(cache_error)?;
        loop {
            match lock.try_lock() {
                Ok(()) => {
                    return Ok(BuildLease {
                        root: root.to_owned(),
                        _lock: lock,
                    })
                }
                Err(TryLockError::WouldBlock) => {
                    tokio::time::sleep(Duration::from_millis(25)).await
                }
                Err(TryLockError::Error(error)) => return Err(cache_error(error)),
            }
        }
    }

    pub(crate) async fn size(&self) -> Result<u64, String> {
        let target = self.root().await?.join("target");
        tokio::task::spawn_blocking(move || size(&target).map_err(cache_error))
            .await
            .map_err(cache_error)?
    }

    pub(crate) async fn clear(&self) -> Result<(), String> {
        let lease = self.acquire().await?;
        tokio::task::spawn_blocking(move || match fs::remove_dir_all(lease.target()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(cache_error(error)),
        })
        .await
        .map_err(cache_error)??;
        self.warming.store(false, Ordering::Release);
        Ok(())
    }

    pub(super) fn schedule_warmup(self: &Arc<Self>) {
        if self.persistent.is_none() || self.warming.swap(true, Ordering::AcqRel) {
            return;
        }
        let cache = self.clone();
        let mut activity = cache.activity.subscribe();
        tokio::spawn(async move {
            let completed = tokio::select! {
                biased;
                _ = activity.changed() => false,
                result = async {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    cache.precompile().await
                } => result.unwrap_or(false),
            };
            if !completed {
                cache.warming.store(false, Ordering::Release);
            }
        });
    }

    async fn precompile(&self) -> Result<bool, String> {
        let root = self.root().await?;
        let lock = open_lock(root).map_err(cache_error)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(false),
            Err(TryLockError::Error(error)) => return Err(cache_error(error)),
        }
        let lease = BuildLease {
            root: root.to_owned(),
            _lock: lock,
        };
        let runtime = lease.runtime()?;
        let project = ShadowProject::temporary(super::source::FILE)?;
        let manifest = Manifest::parse(POPULAR_CRATES, None)?;
        stage(
            &project,
            "fn main() {}",
            &manifest,
            &runtime,
            "codingpad-warmup",
        )?;
        let mut command = Command::new(cargo());
        command
            .current_dir(project.root())
            .args(["build", "--quiet", "--jobs", "1", "--target-dir"])
            .arg(lease.target())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = spawn(&mut command).map_err(cache_error)?;
        if let Ok(Ok(_)) =
            tokio::time::timeout(Duration::from_secs(120), child.process.wait()).await
        {
            child.finished = true;
        }
        Ok(true)
    }
}

pub(super) struct BuildLease {
    root: PathBuf,
    _lock: File,
}

impl BuildLease {
    pub(super) fn target(&self) -> PathBuf {
        self.root.join("target")
    }

    pub(super) fn runtime(&self) -> Result<PathBuf, String> {
        let mut hash = Sha256::new();
        for source in [RUNTIME_MANIFEST, RUNTIME, RICH_RUNTIME, CACHE_RUNTIME] {
            hash.update(source.len().to_le_bytes());
            hash.update(source);
        }
        let runtime = self.root.join(format!("runtime-{:x}", hash.finalize()));
        fs::create_dir_all(runtime.join("src")).map_err(cache_error)?;
        for (file, content) in [
            ("Cargo.toml", RUNTIME_MANIFEST),
            ("src/lib.rs", RUNTIME),
            ("src/rich.rs", RICH_RUNTIME),
            ("src/cache.rs", CACHE_RUNTIME),
        ] {
            write_if_changed(&runtime.join(file), content).map_err(cache_error)?;
        }
        Ok(runtime)
    }
}

pub(super) fn stage(
    project: &ShadowProject,
    source: &str,
    manifest: &Manifest,
    runtime: &Path,
    binary: &str,
) -> Result<(), String> {
    let root = project.root();
    let manifest = manifest.for_project(runtime, binary)?;
    fs::create_dir_all(root.join("src")).map_err(cache_error)?;
    write_if_changed(&root.join("Cargo.toml"), &manifest).map_err(cache_error)?;
    write_if_changed(&root.join(super::source::FILE), source).map_err(cache_error)
}

fn write_if_changed(path: &Path, content: &str) -> io::Result<()> {
    if fs::read(path).is_ok_and(|current| current == content.as_bytes()) {
        return Ok(());
    }
    use io::Write;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    file.write_all(content.as_bytes())?;
    file.persist(path)?;
    Ok(())
}

fn open_lock(root: &Path) -> io::Result<File> {
    fs::create_dir_all(root)?;
    File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("cache.lock"))
}

fn size(path: &Path) -> io::Result<u64> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut bytes = 0u64;
    for entry in entries {
        let entry = entry?;
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if metadata.is_dir() {
            bytes = bytes.saturating_add(size(&entry.path())?);
        } else if metadata.is_file() {
            bytes = bytes.saturating_add(metadata.len());
        }
    }
    Ok(bytes)
}

fn cache_error(error: impl std::fmt::Display) -> String {
    format!("Rust build cache: {error}")
}
