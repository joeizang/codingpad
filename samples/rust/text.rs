use std::collections::BTreeMap;

let mut counts = BTreeMap::new();
for word in "Rust rust Café café coffee".split_whitespace() {
    *counts.entry(word.to_lowercase()).or_insert(0) += 1;
}

counts
