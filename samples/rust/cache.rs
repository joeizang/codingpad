use codingpad::{cache, dump};

fn squares() -> Vec<u32> {
    cache("squares", || {
        println!("Computing squares");
        (1_u32..=8).map(|value| value * value).collect()
    })
}

let first = squares();
let second = squares();
dump((first, second));
