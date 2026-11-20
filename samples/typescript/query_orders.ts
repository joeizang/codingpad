// codingpad: connection=codingpad-sample

import { db } from "codingpad";

// Join customers and line items, then calculate one total per order.
await db.query(
  `SELECT o.id, c.name AS customer, o.ordered_at, o.status,
          ROUND(SUM(i.quantity * i.unit_price), 2) AS total
   FROM orders AS o
   JOIN customers AS c ON c.id = o.customer_id
   JOIN order_items AS i ON i.order_id = o.id
   GROUP BY o.id, c.name, o.ordered_at, o.status
   ORDER BY o.ordered_at`,
);
