use codingpad::{cache, dump};

// Rerun this tab or invoke cpad again on this file to reuse the result.
let squares = cache("squares", || {
    println!("Computing squares");
    (1_u32..=8).map(|value| value * value).collect::<Vec<_>>()
});

dump(squares);
