// codingpad: connection=codingpad-sample
use codingpad::db;

db.query(
    "SELECT p.name AS product, c.name AS category, p.price
     FROM products p
     JOIN categories c ON c.id = p.category_id
     ORDER BY p.price DESC, p.id
     LIMIT 8",
)
.unwrap()
