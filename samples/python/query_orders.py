# codingpad: connection=codingpad-sample

# Join the sample database

orders = db.query(
    """
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
    """
)

dump(orders)

# Parameters use the driver's syntax; SQLite uses ? placeholders.
shipped = db.query(
    "SELECT id, ordered_at FROM orders WHERE status = ? ORDER BY ordered_at",
    ["shipped"],
)
dump(shipped)
