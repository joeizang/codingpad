//! A fatal panic must leave a report for the next launch, even though the
//! process that panicked cannot show anything. A panic on a worker thread is
//! normally caught by the runtime and must not look like a crash.

use codingpad_kernel_core::crash;

fn panic_on(thread: &str) {
    let caught = std::thread::Builder::new()
        .name(thread.into())
        .spawn(|| {
            let _ = std::panic::catch_unwind(|| panic!("forced crash for the test"));
        })
        .unwrap()
        .join();
    assert!(caught.is_ok(), "the test panic should be caught");
}

#[test]
fn only_a_main_thread_panic_is_a_crash() {
    let directory = tempfile::tempdir().unwrap();
    crash::install_panic_hook(directory.path().to_path_buf());

    panic_on("worker");
    assert_eq!(crash::take_pending(directory.path()), None);

    panic_on("main");
    let details = crash::take_pending(directory.path()).expect("the main panic was recorded");
    assert!(details.message.contains("forced crash for the test"));
    assert!(details.location.is_some());
    assert!(details.backtrace.is_some());
    // Taking it once is enough; the same crash is not offered twice.
    assert_eq!(crash::take_pending(directory.path()), None);
}

#[test]
fn a_clean_exit_discards_the_record() {
    let directory = tempfile::tempdir().unwrap();
    crash::write_pending(
        directory.path(),
        &crash::CrashDetails::new("boom", None, None),
    )
    .unwrap();
    assert!(directory.path().join(crash::PENDING_FILE).is_file());
    crash::clear_pending(directory.path());
    assert!(!directory.path().join(crash::PENDING_FILE).exists());
}
