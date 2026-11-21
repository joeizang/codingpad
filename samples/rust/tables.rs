#[derive(Debug)]
struct Book {
    title: &'static str,
    pages: u32,
}

let books = vec![
    Book { title: "Pride and Prejudice", pages: 432 },
    Book { title: "The Time Machine", pages: 118 },
    Book { title: "The Secret Garden", pages: 331 },
];

println!("{} books, {} pages", books.len(), books.iter().map(|book| book.pages).sum::<u32>());
println!("First title: {}", books[0].title);

// A sequence of records with matching fields becomes a table.
books
