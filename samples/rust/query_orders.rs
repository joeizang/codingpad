// codingpad: connection=codingpad-sample
use codingpad::db;

db.query(
    "SELECT o.id AS order_id, c.name AS customer, o.ordered_at,
            ROUND(SUM(i.quantity * i.unit_price), 2) AS total
     FROM orders o
     JOIN customers c ON c.id = o.customer_id
     JOIN order_items i ON i.order_id = o.id
     GROUP BY o.id, c.name, o.ordered_at
     ORDER BY total DESC, o.id
     LIMIT 8",
)
.unwrap()
