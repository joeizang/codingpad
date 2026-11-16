//! Real OS password vault round trip (M3 close, issue #48). Run on each target
//! platform with `cargo test -p codingpad-kernel-core --test vault -- --ignored`:
//! Secret Service on Linux, Credential Manager on Windows.

use codingpad_kernel_core::secrets::{SecretOrigin, SecretStore};
use codingpad_kernel_core::{KernelEvent, Language, Supervisor};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::timeout;

async fn stdout(supervisor: &Supervisor, language: Language, source: &str) -> String {
    let mut events: Vec<KernelEvent> = Vec::new();
    let result = timeout(
        Duration::from_secs(60),
        supervisor.run_language("vault", source, language, |event| events.push(event)),
    )
    .await
    .expect("run timed out")
    .expect("kernel failed");
    assert!(result.ok, "{events:?}");
    events
        .iter()
        .filter(|event| event.method == "stream" && event.params["stream"] == "stdout")
        .filter_map(|event| event.params["text"].as_str())
        .collect()
}

#[tokio::test]
#[ignore = "writes to the real OS password vault; run explicitly on each platform"]
async fn secrets_round_trip_through_the_os_vault_into_both_kernels() {
    let name = "codingpad_m3_vault_check";
    let value = format!(
        "v{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let directory = tempfile::tempdir().unwrap();
    let store = SecretStore::new(directory.path());
    assert_ne!(
        store.vault_available(),
        Some(false),
        "OS vault reported unavailable"
    );
    assert_eq!(store.set(name, &value).unwrap(), SecretOrigin::Vault);

    // A fresh store has no cache, as after an app restart: the value must come
    // back from the OS vault itself.
    let restarted = Arc::new(SecretStore::new(directory.path()));
    assert_eq!(restarted.get(name).unwrap(), value);
    let supervisor =
        Supervisor::with_data_dir_and_secrets(directory.path().to_path_buf(), restarted.clone());
    let python = stdout(
        &supervisor,
        Language::Python,
        &format!("from codingpad import secret\nprint(secret({name:?}) == {value:?})"),
    )
    .await;
    let csharp = stdout(
        &supervisor,
        Language::Csharp,
        &format!("Console.WriteLine(Secret({name:?}) == {value:?});"),
    )
    .await;
    supervisor.shutdown().await;
    restarted.delete(name, false).unwrap();
    let after = SecretStore::new(directory.path());
    let missing = after.get(name);
    assert_eq!(python.trim(), "True");
    assert_eq!(csharp.trim(), "True");
    assert!(
        missing.is_err(),
        "deleted secret is still readable: {missing:?}"
    );
}
