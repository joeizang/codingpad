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

const TARGET_DIR: &str = "rust-build";
const POPULAR_CRATES: &[(&str, &str)] = &[
    ("serde", "1"),
    ("serde_json", "1"),
    ("regex", "1"),
    ("anyhow", "1"),
];

#[derive(Clone, Copy)]
enum Access {
    Shared,
    Exclusive,
}

pub(crate) struct BuildCache {
    data: Option<PathBuf>,
    temporary: OnceCell<TempDir>,
    activity: watch::Sender<()>,
    warmup_scheduled_or_complete: AtomicBool,
}

impl BuildCache {
    pub(crate) fn new(data: Option<&Path>) -> Self {
        Self {
            data: data.map(Path::to_owned),
            temporary: OnceCell::new(),
            activity: watch::channel(()).0,
            warmup_scheduled_or_complete: AtomicBool::new(false),
        }
    }

    async fn data_root(&self) -> Result<&Path, String> {
        if let Some(root) = &self.data {
            return Ok(root);
        }
        let temp = self
            .temporary
            .get_or_try_init(|| async { tempfile::tempdir().map_err(cache_error) })
            .await?;
        Ok(temp.path())
    }

    pub(super) async fn acquire(&self) -> Result<BuildLease, String> {
        self.lock(Access::Shared).await
    }

    async fn lock(&self, access: Access) -> Result<BuildLease, String> {
        self.activity.send_replace(());
        let root = self.data_root().await?;
        let lock = open_lock(root).map_err(cache_error)?;
        loop {
            match try_acquire(&lock, access) {
                Ok(true) => {
                    return Ok(BuildLease {
                        target: root.join(TARGET_DIR),
                        _lock: lock,
                    })
                }
                Ok(false) => tokio::time::sleep(Duration::from_millis(25)).await,
                Err(error) => return Err(cache_error(error)),
            }
        }
    }

    pub(crate) async fn size(&self) -> Result<u64, String> {
        let target = self.data_root().await?.join(TARGET_DIR);
        tokio::task::spawn_blocking(move || size(&target).map_err(cache_error))
            .await
            .map_err(cache_error)?
    }

    pub(crate) async fn clear(&self) -> Result<(), String> {
        let lease = self.lock(Access::Exclusive).await?;
        tokio::task::spawn_blocking(move || match fs::remove_dir_all(lease.target()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(cache_error(error)),
        })
        .await
        .map_err(cache_error)??;
        self.warmup_scheduled_or_complete
            .store(false, Ordering::Release);
        Ok(())
    }

    pub(super) fn schedule_warmup(self: &Arc<Self>) {
        if self.data.is_none()
            || self
                .warmup_scheduled_or_complete
                .swap(true, Ordering::AcqRel)
        {
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
                cache
                    .warmup_scheduled_or_complete
                    .store(false, Ordering::Release);
            }
        });
    }

    async fn precompile(&self) -> Result<bool, String> {
        let root = self.data_root().await?;
        let lock = open_lock(root).map_err(cache_error)?;
        if !try_acquire(&lock, Access::Exclusive).map_err(cache_error)? {
            return Ok(false);
        }
        let lease = BuildLease {
            target: root.join(TARGET_DIR),
            _lock: lock,
        };
        let runtime = self.stage_runtime().await?;
        let project = ShadowProject::temporary(super::source::FILE)?;
        let result = tokio::time::timeout(Duration::from_secs(120), async {
            let mut all_compiled = true;
            for (name, version) in POPULAR_CRATES {
                let source = format!("---cargo\n[dependencies]\n{name} = {version:?}\n---\n");
                let manifest = Manifest::parse(&source, None).map_err(|error| error.to_string())?;
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
                    .args([
                        "build",
                        "--offline",
                        "--quiet",
                        "--jobs",
                        "1",
                        "--target-dir",
                    ])
                    .arg(lease.target())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                let mut child = spawn(&mut command).map_err(cache_error)?;
                let status = child.process.wait().await.map_err(cache_error)?;
                child.finished = true;
                all_compiled &= status.success();
            }
            Ok(all_compiled)
        })
        .await;
        result.unwrap_or(Ok(false))
    }

    pub(super) async fn stage_runtime(&self) -> Result<PathBuf, String> {
        let root = self.data_root().await?;
        let mut hash = Sha256::new();
        for source in [RUNTIME_MANIFEST, RUNTIME, RICH_RUNTIME, CACHE_RUNTIME] {
            hash.update(source.len().to_le_bytes());
            hash.update(source);
        }
        let runtime = root
            .join("rust-runtime")
            .join(format!("{:x}", hash.finalize()));
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

pub(super) struct BuildLease {
    target: PathBuf,
    _lock: File,
}

impl BuildLease {
    pub(super) fn target(&self) -> &Path {
        &self.target
    }
}

fn try_acquire(lock: &File, access: Access) -> io::Result<bool> {
    match match access {
        Access::Shared => lock.try_lock_shared(),
        Access::Exclusive => lock.try_lock(),
    } {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(error)) => Err(error),
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
        .open(root.join("rust-build.lock"))
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
