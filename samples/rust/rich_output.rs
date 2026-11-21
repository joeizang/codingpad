use codingpad::{dump, html, live};
use std::{thread, time::Duration};

let progress = live("Starting");
dump(&progress);

for status in ["Reading", "Summarizing", "Done"] {
    thread::sleep(Duration::from_millis(100));
    progress.set(status);
}

dump(html("<strong>All three steps completed.</strong>"));
