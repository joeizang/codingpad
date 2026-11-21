use super::{
    manifest::Manifest, spawn, CACHE_RUNTIME, DB_RUNTIME, RICH_RUNTIME, RUNTIME, RUNTIME_MANIFEST,
};
use crate::{
    shadow::ShadowProject,
    toolchains::{RustTool, RustToolchain, RustToolchainError},
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, TryLockError},
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tempfile::TempDir;
use tokio::{
    process::Command,
    sync::{watch, OnceCell},
};

const TARGET_DIR: &str = "rust-build";
const PRIVATE_TOOLCHAIN_RECHECK: Duration = Duration::from_secs(30);
const CLEAR_WAIT: Duration = Duration::from_secs(1);
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
    warmup_scheduled: AtomicBool,
    toolchain: tokio::sync::Mutex<Option<(RustToolchain, Instant)>>,
    precompiled_crates: Mutex<HashSet<&'static str>>,
}

impl BuildCache {
    pub(crate) fn new(data: Option<&Path>) -> Self {
        Self {
            data: data.map(Path::to_owned),
            temporary: OnceCell::new(),
            activity: watch::channel(()).0,
            warmup_scheduled: AtomicBool::new(false),
            toolchain: tokio::sync::Mutex::default(),
            precompiled_crates: Mutex::default(),
        }
    }

    pub(super) async fn toolchain(&self) -> Result<RustToolchain, RustToolchainError> {
        let mut cached = self.toolchain.lock().await;
        if let Some((toolchain, resolved)) = &*cached {
            // Recheck a private selection periodically so new user installs
            // can take precedence. Missing files always trigger a recheck.
            if toolchain.still_present()
                && (!toolchain.is_private() || resolved.elapsed() < PRIVATE_TOOLCHAIN_RECHECK)
            {
                return Ok(toolchain.clone());
            }
        }
        let toolchain = RustToolchain::resolve(self.data.as_deref()).await?;
        *cached = Some((toolchain.clone(), Instant::now()));
        Ok(toolchain)
    }

    pub(super) async fn invalidate_toolchain(&self) {
        self.toolchain.lock().await.take();
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
        let lease = BuildLease::open(self.data_root().await?).map_err(cache_error)?;
        while !lease.try_acquire(access).map_err(cache_error)? {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        Ok(lease)
    }

    pub(crate) async fn size(&self) -> Result<u64, String> {
        let target = self.data_root().await?.join(TARGET_DIR);
        tokio::task::spawn_blocking(move || size(&target).map_err(cache_error))
            .await
            .map_err(cache_error)?
    }

    pub(crate) async fn clear(&self) -> Result<(), String> {
        let lease = tokio::time::timeout(CLEAR_WAIT, self.lock(Access::Exclusive))
            .await
            .map_err(|_| {
                "The Rust build cache is in use. Finish or stop active Rust scripts and try again."
                    .to_owned()
            })??;
        // Reset progress under the lease so an older warmup cannot write it back.
        self.precompiled_crates
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        tokio::task::spawn_blocking(move || match fs::remove_dir_all(lease.target()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(cache_error(error)),
        })
        .await
        .map_err(cache_error)?
    }

    pub(super) fn schedule_warmup(self: &Arc<Self>) {
        if self
            .precompiled_crates
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
            == POPULAR_CRATES.len()
            || self.warmup_scheduled.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let cache = self.clone();
        let mut activity = cache.activity.subscribe();
        tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = activity.changed() => {}
                _ = async {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    if let Err(error) = cache.precompile().await {
                        eprintln!("Rust build cache precompilation: {error}");
                    }
                } => {}
            }
            cache.warmup_scheduled.store(false, Ordering::Release);
        });
    }

    async fn precompile(&self) -> Result<(), String> {
        let toolchain = self.toolchain().await?;
        let lease = BuildLease::open(self.data_root().await?).map_err(cache_error)?;
        if !lease.try_acquire(Access::Exclusive).map_err(cache_error)? {
            return Ok(());
        }
        let runtime = self.stage_runtime().await?;
        let project = ShadowProject::temporary(super::source::FILE)?;
        tokio::time::timeout(Duration::from_secs(120), async {
            for &(name, version) in POPULAR_CRATES {
                if self
                    .precompiled_crates
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .contains(name)
                {
                    continue;
                }
                let source = format!("---cargo\n[dependencies]\n{name} = {version:?}\n---\n");
                let manifest = Manifest::parse(&source, None).map_err(|error| error.to_string())?;
                stage(
                    &project,
                    "fn main() {}",
                    &manifest,
                    &runtime,
                    "codingpad-warmup",
                )?;
                let mut command = toolchain.command(RustTool::Cargo);
                command.current_dir(project.root()).args([
                    "build",
                    "--offline",
                    "--quiet",
                    "--jobs",
                    "1",
                ]);
                use_shared_target(&mut command, lease.target());
                if run_in_background(command).await? {
                    self.precompiled_crates
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .insert(name);
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| "Rust build cache warmup timed out".to_owned())?
    }

    pub(super) async fn stage_runtime(&self) -> Result<PathBuf, String> {
        let root = self.data_root().await?;
        let mut hash = Sha256::new();
        for source in [
            RUNTIME_MANIFEST,
            RUNTIME,
            RICH_RUNTIME,
            CACHE_RUNTIME,
            DB_RUNTIME,
        ] {
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
            ("src/database.rs", DB_RUNTIME),
        ] {
            write_if_changed(&runtime.join(file), content).map_err(cache_error)?;
        }
        Ok(runtime)
    }
}

async fn run_in_background(mut command: Command) -> Result<bool, String> {
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = spawn(&mut command).map_err(cache_error)?;
    let status = child.process.wait().await.map_err(cache_error)?;
    child.finished = true;
    Ok(status.success())
}

pub(super) struct BuildLease {
    target: PathBuf,
    file: File,
}

impl BuildLease {
    fn open(root: &Path) -> io::Result<Self> {
        Ok(Self {
            target: root.join(TARGET_DIR),
            file: open_lock(root)?,
        })
    }

    fn try_acquire(&self, access: Access) -> io::Result<bool> {
        match match access {
            Access::Shared => self.file.try_lock_shared(),
            Access::Exclusive => self.file.try_lock(),
        } {
            Ok(()) => Ok(true),
            Err(TryLockError::WouldBlock) => Ok(false),
            Err(TryLockError::Error(error)) => Err(error),
        }
    }

    pub(super) fn target(&self) -> &Path {
        &self.target
    }
}

pub(super) fn use_shared_target(command: &mut Command, target: &Path) {
    // Cargo can place intermediate and final artifacts in different directories.
    // Keep both in the cache even when the user's config sets a build directory.
    command
        .arg("--target-dir")
        .arg(target)
        .env("CARGO_BUILD_BUILD_DIR", target);
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

pub(super) fn write_if_changed(path: &Path, content: &str) -> io::Result<()> {
    if fs::read(path).is_ok_and(|current| current == content.as_bytes()) {
        return Ok(());
    }
    use io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing output directory"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn clear_reports_a_busy_cache_and_can_retry() {
        let directory = tempfile::tempdir().unwrap();
        let cache = BuildCache::new(Some(directory.path()));
        let lease = cache.acquire().await.unwrap();
        fs::create_dir(lease.target()).unwrap();
        let artifact = b"compiled";
        fs::write(lease.target().join("artifact"), artifact).unwrap();

        let failure = cache.clear().await.unwrap_err();
        assert!(
            failure.contains("The Rust build cache is in use"),
            "{failure}"
        );
        assert_eq!(cache.size().await.unwrap(), artifact.len() as u64);
        drop(lease);

        cache.clear().await.unwrap();
        assert_eq!(cache.size().await.unwrap(), 0);
    }
}
