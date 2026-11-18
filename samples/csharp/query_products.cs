// codingpad: connection=sample

// Query the sample database

db.Query("""
    SELECT p.name AS product,
           c.name AS category,
           p.price,
           p.stock
    FROM products AS p
    JOIN categories AS c ON c.id = p.category_id
    WHERE p.discontinued = 0
    ORDER BY c.name, p.name
    """).Dump();
