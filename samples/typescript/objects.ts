import { dump } from "codingpad";

interface Book {
  title: string;
  author: { name: string; country: string };
  tags: string[];
}

const book: Book = {
  title: "Things Fall Apart",
  author: { name: "Chinua Achebe", country: "Nigeria" },
  tags: ["fiction", "classic"],
};

// Expand the author or tags in Results to inspect the nested values.
dump(book);
