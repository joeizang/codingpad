//! A panic must leave a report for the next launch, even though the process
//! that panicked cannot show anything.

use codingpad_kernel_core::crash;

#[test]
fn a_panic_leaves_a_report_for_the_next_launch() {
    let directory = tempfile::tempdir().unwrap();
    crash::install_panic_hook(directory.path().to_path_buf());
    let caught = std::panic::catch_unwind(|| panic!("forced crash for the test"));
    assert!(caught.is_err());
    let details = crash::take_pending(directory.path()).expect("the panic was recorded");
    assert!(details.message.contains("forced crash for the test"));
    assert!(details.location.is_some());
    assert!(details.backtrace.is_some());
    // Taking it once is enough; the same crash is not offered twice.
    assert_eq!(crash::take_pending(directory.path()), None);
}
