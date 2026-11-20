// codingpad: connection=codingpad-sample

import { db } from "codingpad";

// Bind a value with ? and pass it separately from the SQL.
await db.query(
  `SELECT p.name AS product, c.name AS category, p.price, p.stock
   FROM products AS p
   JOIN categories AS c ON c.id = p.category_id
   WHERE p.discontinued = 0 AND p.stock >= ?
   ORDER BY c.name, p.name`,
  [5],
);
