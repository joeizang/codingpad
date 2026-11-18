// codingpad: connection=codingpad-sample

// Join the sample database

db.Query("""
    SELECT o.id,
           c.name AS customer,
           o.ordered_at,
           o.status,
           printf('%.2f', SUM(i.quantity * i.unit_price)) AS total
    FROM orders AS o
    JOIN customers AS c ON c.id = o.customer_id
    JOIN order_items AS i ON i.order_id = o.id
    GROUP BY o.id, c.name, o.ordered_at, o.status
    ORDER BY o.ordered_at
    """).Dump();

// Parameterised queries take a dictionary keyed by the database parameter
// name; this sample filters with a literal to keep the statement simple.
db.Query(
    "SELECT id, ordered_at FROM orders WHERE status = 'shipped' ORDER BY ordered_at"
).Dump();
