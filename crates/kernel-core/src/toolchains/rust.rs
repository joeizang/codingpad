use super::{
    installer, prepare_install, prepare_tool_command, probe_version, promote_directory, Artifact,
    Toolchains, MAX_TOOL_BYTES,
};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

const RUSTUP_VERSION: &str = "1.29.3";

// Published SHA-256 digests from the corresponding rustup archive directory.
const RUSTUP_RELEASES: &[(&str, &str)] = &[
    (
        "x86_64-unknown-linux-gnu",
        "b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55",
    ),
    (
        "aarch64-unknown-linux-gnu",
        "9f608c142e0e38a78957d0661ba1dfaf0b6fb37184cfaa4b9a0aa57af6184282",
    ),
    (
        "x86_64-unknown-linux-musl",
        "1552169aba2836777136ccff7a381e95c17233b5d8250cd5e10ae3cb971bfcfc",
    ),
    (
        "aarch64-unknown-linux-musl",
        "956deacdc27ab82ae7ac821d5d3e9e95e4795d476d17efe99743bb8531ca1031",
    ),
    (
        "x86_64-apple-darwin",
        "bc107f1fb0228de7fa32413c89c1b305cea9cd256e94b01150cd054128145429",
    ),
    (
        "aarch64-apple-darwin",
        "63752a1919a3a4793173ec70f8f212621a7ce38ba6a246095984482f4de20f9f",
    ),
    (
        "x86_64-pc-windows-msvc",
        "38027d51d69a5d05493627a0634dbea52d6cd793c8f072d2373d3f4163eec7db",
    ),
];

const MISSING: &str = "Rust was not found. Install Rust from Results in CodingPad, or install a stable toolchain from https://rustup.rs/ and try again.";

pub(crate) enum RustToolchainError {
    Missing(Option<String>),
    ProbeTimeout,
    Other(String),
}

impl From<String> for RustToolchainError {
    fn from(error: String) -> Self {
        Self::Other(error)
    }
}

impl From<RustToolchainError> for String {
    fn from(error: RustToolchainError) -> Self {
        match error {
            RustToolchainError::Missing(Some(detail)) => format!("{MISSING}\n{detail}"),
            RustToolchainError::Missing(None) => MISSING.into(),
            RustToolchainError::ProbeTimeout => PROBE_TIMEOUT.into(),
            RustToolchainError::Other(error) => error,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum RustTool {
    Cargo,
    Compiler,
    Analyzer,
    Rustup,
}

impl RustTool {
    fn filename(self) -> String {
        executable_name(match self {
            Self::Cargo => "cargo",
            Self::Compiler => "rustc",
            Self::Analyzer => "rust-analyzer",
            Self::Rustup => "rustup",
        })
    }
}

#[derive(Clone)]
pub(crate) struct RustToolchain {
    private: Option<PathBuf>,
    cargo: PathBuf,
    compiler: PathBuf,
}

impl Default for RustToolchain {
    fn default() -> Self {
        Self {
            private: None,
            cargo: find_user_tool(RustTool::Cargo),
            compiler: find_user_tool(RustTool::Compiler),
        }
    }
}

impl RustToolchain {
    fn private(root: PathBuf) -> Self {
        let bin = root.join("cargo/bin");
        Self {
            private: Some(root),
            cargo: bin.join(RustTool::Cargo.filename()),
            compiler: bin.join(RustTool::Compiler.filename()),
        }
    }

    pub(crate) fn is_private(&self) -> bool {
        self.private.is_some()
    }

    pub(crate) fn analyzer_help(&self) -> String {
        self.private.as_ref().map_or_else(
            || "rust-analyzer was not found. Install it with `rustup component add rust-analyzer rust-src` or set CODINGPAD_RUST_LSP.".into(),
            |root| format!(
                "rust-analyzer was not found in CodingPad's private Rust toolchain. Set CARGO_HOME to \"{}\" and RUSTUP_HOME to \"{}\", then run \"{}\" component add rust-analyzer rust-src.",
                root.join("cargo").display(),
                root.join("rustup").display(),
                self.executable(RustTool::Rustup).display(),
            ),
        )
    }

    pub(crate) fn still_present(&self) -> bool {
        self.cargo.is_file() && self.compiler.is_file()
    }

    pub(crate) async fn resolve(data: Option<&Path>) -> Result<Self, RustToolchainError> {
        let user = Self::default();
        let user_ready = user.is_available().await;
        if user_ready == Some(true) {
            return Ok(user);
        }
        if let Some(data) = data {
            let private =
                Self::private(std::path::absolute(private_dir(data)).map_err(|e| e.to_string())?);
            match private.is_available().await {
                Some(true) => return Ok(private),
                None => return Err(RustToolchainError::ProbeTimeout),
                Some(false) => {}
            }
        }
        if user_ready.is_none() {
            return Err(RustToolchainError::ProbeTimeout);
        }
        Err(RustToolchainError::Missing(None))
    }

    pub(crate) fn executable(&self, tool: RustTool) -> PathBuf {
        match tool {
            RustTool::Cargo => self.cargo.clone(),
            RustTool::Compiler => self.compiler.clone(),
            RustTool::Analyzer | RustTool::Rustup => self.private.as_ref().map_or_else(
                || find_user_tool(tool),
                |root| root.join("cargo/bin").join(tool.filename()),
            ),
        }
    }

    pub(crate) fn command(&self, tool: RustTool) -> Command {
        let mut command = Command::new(self.executable(tool));
        self.configure(&mut command);
        command
    }

    pub(crate) fn configure(&self, command: &mut Command) {
        if let Some(root) = &self.private {
            private_homes(command, root);
            for key in [
                "RUSTUP_TOOLCHAIN",
                "RUSTC_WRAPPER",
                "RUSTC_WORKSPACE_WRAPPER",
                "RUSTFLAGS",
                "CARGO_ENCODED_RUSTFLAGS",
                "CARGO_BUILD_TARGET",
                "CARGO_BUILD_RUSTC",
            ] {
                command.env_remove(key);
            }
            command.env("RUSTC", &self.compiler);
            command.env(
                "RUSTDOC",
                root.join("cargo/bin").join(executable_name("rustdoc")),
            );
        }
        if let Some(bin) = self.cargo.parent().filter(|path| path.is_absolute()) {
            let path = std::env::var_os("PATH").unwrap_or_default();
            if let Ok(paths) = std::env::join_paths(
                std::iter::once(bin.to_owned()).chain(std::env::split_paths(&path)),
            ) {
                command.env("PATH", paths);
            }
        }
    }

    async fn is_available(&self) -> Option<bool> {
        if !self.still_present() {
            return Some(false);
        }
        let (cargo, rustc) = tokio::join!(
            probe_version(self.command(RustTool::Cargo), VERSION_TIMEOUT),
            probe_version(self.command(RustTool::Compiler), VERSION_TIMEOUT)
        );
        Some(cargo? && rustc?)
    }
}

const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: &str =
    "The Rust toolchain took too long to respond. Wait for it to finish starting, then try again.";

fn find_user_tool(tool: RustTool) -> PathBuf {
    let name = tool.filename();
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(std::env::var_os("CARGO_HOME").map(|path| PathBuf::from(path).join("bin")))
        .chain(dirs::home_dir().map(|home| home.join(".cargo/bin")))
        .map(|dir| dir.join(&name))
        .find(|path| path.is_file())
        .and_then(|path| std::path::absolute(path).ok())
        .unwrap_or_else(|| PathBuf::from(name))
}

fn private_dir(data: &Path) -> PathBuf {
    data.join("toolchains/rust/stable")
}

fn private_homes(command: &mut Command, root: &Path) {
    command
        .env("CARGO_HOME", root.join("cargo"))
        .env("RUSTUP_HOME", root.join("rustup"));
}

fn executable_name(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

struct RustupRelease {
    bootstrap: &'static str,
    host: &'static str,
    sha256: &'static str,
}

fn release(os: &str, arch: &str, musl: bool) -> Result<RustupRelease, String> {
    let unsupported = || {
        format!("Private Rust installation is unavailable on {os}/{arch}. Install a Rust toolchain from https://rustup.rs/ and try again.")
    };
    let platform = match os {
        "linux" if musl => "unknown-linux-musl",
        "linux" => "unknown-linux-gnu",
        "macos" => "apple-darwin",
        "windows" if arch == "x86_64" => "pc-windows-msvc",
        _ => return Err(unsupported()),
    };
    let platform = format!("{arch}-{platform}");
    let (triple, sha256) = RUSTUP_RELEASES
        .iter()
        .copied()
        .find(|(target, _)| *target == platform)
        .ok_or_else(unsupported)?;
    // The x64 GNU distribution includes its linker and Windows import libraries.
    let host = if os == "windows" {
        "x86_64-pc-windows-gnu"
    } else {
        triple
    };
    Ok(RustupRelease {
        bootstrap: triple,
        host,
        sha256,
    })
}

impl Toolchains {
    pub async fn install_rust(&self, progress: impl FnMut(&str) + Send) -> Result<(), String> {
        let _guard = self.install.lock().await;
        match RustToolchain::resolve(Some(&self.data_dir)).await {
            Ok(_) => Ok(()),
            Err(RustToolchainError::Missing(_)) => {
                install_private_rust(&self.data_dir, progress).await
            }
            Err(error) => Err(error.into()),
        }
    }
}

async fn install_private_rust(
    data_dir: &Path,
    mut progress: impl FnMut(&str) + Send,
) -> Result<(), String> {
    let release = release(
        std::env::consts::OS,
        std::env::consts::ARCH,
        cfg!(target_env = "musl"),
    )?;
    let destination = std::path::absolute(private_dir(data_dir)).map_err(|e| e.to_string())?;
    let root = destination.parent().ok_or("Missing Rust tool directory")?;
    let _lock = prepare_install(root)?;
    let private = RustToolchain::private(destination.clone());
    match private.is_available().await {
        Some(true) => return Ok(()),
        None => return Err(PROBE_TIMEOUT.into()),
        Some(false) => {}
    }
    let stage = tempfile::Builder::new()
        .prefix(".codingpad-install-rust-")
        .tempdir_in(root)
        .map_err(|e| format!("Could not create the Rust staging directory: {e}"))?;
    progress("Checking the system linker…");
    verify_linker(stage.path()).await?;
    if destination.exists() {
        std::fs::remove_dir_all(&destination)
            .map_err(|e| format!("Could not remove the unusable private Rust toolchain: {e}"))?;
    }

    let name = executable_name("rustup-init");
    progress("Downloading and verifying rustup…");
    let bootstrap = installer::download_verified(
        &Artifact {
            url: &format!(
                "https://static.rust-lang.org/rustup/archive/{RUSTUP_VERSION}/{}/{name}",
                release.bootstrap
            ),
            sha256: release.sha256,
            max_bytes: MAX_TOOL_BYTES,
        },
        stage.path(),
    )
    .await?;
    // rustup chooses installer mode from the exact executable name.
    let bootstrap = bootstrap
        .persist(stage.path().join(&name))
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        bootstrap
            .set_permissions(std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    drop(bootstrap);
    let mut command = Command::new(stage.path().join(&name));
    private_homes(&mut command, stage.path());
    command
        .current_dir(stage.path())
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("RUSTUP_UPDATE_ROOT")
        .env_remove("RUSTUP_DIST_SERVER")
        .env_remove("RUSTUP_OVERRIDE_HOST_TRIPLE")
        .env("RUSTUP_INIT_SKIP_PATH_CHECK", "yes")
        .args([
            "--yes",
            "--no-modify-path",
            "--default-host",
            release.host,
            "--default-toolchain",
            "stable",
            "--profile",
            "minimal",
        ]);

    progress("Installing stable Rust…");
    run(command, Duration::from_secs(30 * 60)).await?;
    progress("Checking that Rust can compile and run a script…");
    let staged = RustToolchain::private(stage.path().to_owned());
    verify_install(&staged, stage.path()).await?;
    progress("Installing Rust editor support…");
    let mut components = staged.command(RustTool::Rustup);
    components
        .current_dir(stage.path())
        .env_remove("RUSTUP_DIST_SERVER")
        .args(["component", "add", "rust-analyzer", "rust-src"]);
    let editor = run(components, Duration::from_secs(5 * 60)).await.is_ok();
    std::fs::remove_file(stage.path().join(name))
        .map_err(|error| format!("Could not remove the rustup bootstrap: {error}"))?;
    promote_directory(stage.path(), &destination, "Rust").await?;
    progress(if editor {
        "Rust installed. Starting script…"
    } else {
        "Rust installed. Editor support could not be installed. Starting script…"
    });
    Ok(())
}

async fn verify_linker(directory: &Path) -> Result<(), String> {
    if cfg!(windows) {
        return Ok(());
    }
    let message = if cfg!(target_os = "macos") {
        "Install the Xcode Command Line Tools with xcode-select --install, then retry."
    } else {
        "Install a C compiler and linker (for example, build-essential on Debian or Ubuntu), then retry."
    };
    if cfg!(target_os = "macos") {
        let mut xcode = Command::new("xcode-select");
        xcode.arg("-p");
        run(xcode, VERSION_TIMEOUT).await.map_err(|_| message)?;
    }
    let probe = tempfile::tempdir_in(directory).map_err(|e| e.to_string())?;
    let source = probe.path().join("linker.c");
    std::fs::write(&source, "int main(void) { return 0; }").map_err(|e| e.to_string())?;
    let mut link = Command::new("cc");
    link.current_dir(probe.path())
        .arg(source)
        .arg("-o")
        .arg(probe.path().join("linker"));
    run(link, VERSION_TIMEOUT)
        .await
        .map_err(|error| format!("{message} {error}"))
}

async fn verify_install(toolchain: &RustToolchain, directory: &Path) -> Result<(), String> {
    let probe = tempfile::tempdir_in(directory).map_err(|e| e.to_string())?;
    let src = probe.path().join("src");
    std::fs::create_dir(&src).map_err(|e| e.to_string())?;
    std::fs::write(
        probe.path().join("Cargo.toml"),
        "[package]\nname = \"codingpad-rust-probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(src.join("main.rs"), "fn main() {}").map_err(|e| e.to_string())?;
    let mut cargo = toolchain.command(RustTool::Cargo);
    cargo
        .current_dir(probe.path())
        .args(["run", "--offline", "--quiet"]);
    run(cargo, Duration::from_secs(60))
        .await
        .map_err(|error| format!("Rust could not compile and run a test script. {error}"))
}

async fn run(mut command: Command, timeout: Duration) -> Result<(), String> {
    command.stdin(Stdio::null());
    prepare_tool_command(&mut command);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| "The Rust command timed out.")?
        .map_err(|e| format!("Could not start the Rust command: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Rust command failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "downloads stable Rust and its editor components"]
    async fn private_install_recovers_and_compiles_after_promotion() {
        let data = tempfile::tempdir().unwrap();
        let root = private_dir(data.path());
        let abandoned = root.with_file_name(".codingpad-install-rust-abandoned");
        std::fs::create_dir_all(&abandoned).unwrap();
        std::fs::write(abandoned.join("partial"), b"interrupted").unwrap();
        std::fs::create_dir_all(root.join("cargo/bin")).unwrap();

        install_private_rust(data.path(), |message| eprintln!("{message}"))
            .await
            .unwrap();
        assert!(!abandoned.exists());
        let private = RustToolchain::private(root);
        verify_install(&private, data.path()).await.unwrap();
        assert_eq!(
            probe_version(private.command(RustTool::Analyzer), VERSION_TIMEOUT).await,
            Some(true)
        );
    }

    #[test]
    fn supported_platforms_have_pinned_rustup_artifacts() {
        for (os, arch, musl) in [
            ("linux", "x86_64", false),
            ("linux", "x86_64", true),
            ("linux", "aarch64", false),
            ("linux", "aarch64", true),
            ("macos", "x86_64", false),
            ("macos", "aarch64", false),
            ("windows", "x86_64", false),
        ] {
            let RustupRelease {
                bootstrap,
                host,
                sha256: digest,
            } = release(os, arch, musl).unwrap();
            assert!(bootstrap.starts_with(arch));
            assert_eq!(digest.len(), 64);
            assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
            if os == "windows" {
                assert_eq!(host, "x86_64-pc-windows-gnu");
            } else {
                assert_eq!(bootstrap, host);
            }
        }
        assert!(release("windows", "aarch64", false).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn version_checks_distinguish_timeout_from_failure() {
        let mut slow = Command::new("sh");
        slow.args(["-c", "exec sleep 1"]);
        let mut broken = Command::new("sh");
        broken.args(["-c", "exit 1"]);
        let directory = tempfile::tempdir().unwrap();
        let unlaunchable = directory.path().join("cargo");
        std::fs::write(&unlaunchable, "not a Rust tool").unwrap();
        assert_eq!(
            probe_version(Command::new(unlaunchable), VERSION_TIMEOUT).await,
            Some(false)
        );
        assert_eq!(probe_version(slow, Duration::from_millis(10)).await, None);
        assert_eq!(probe_version(broken, VERSION_TIMEOUT).await, Some(false));
    }
}
