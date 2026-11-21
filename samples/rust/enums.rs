use codingpad::dump;

fn parse_score(text: &str) -> Result<u8, String> {
    match text.parse::<u8>() {
        Ok(score) if score <= 100 => Ok(score),
        Ok(_) => Err("Score exceeds 100".to_owned()),
        Err(_) => Err(format!("Invalid score: {text}")),
    }
}

for text in ["83", "oops", "101"] {
    dump(parse_score(text));
}
