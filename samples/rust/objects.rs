use codingpad::dump;

#[derive(Debug)]
struct User {
    name: &'static str,
    interests: Vec<&'static str>,
    team: Option<&'static str>,
}

let user = User {
    name: "Ada",
    interests: vec!["mathematics", "music"],
    team: Some("Research"),
};

println!("Meet {} from {:?}.", user.name, user.team);
println!("Interests: {}", user.interests.len());
dump(user);
