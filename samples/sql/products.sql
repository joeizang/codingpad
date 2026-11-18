-- codingpad: connection=codingpad-sample

-- Products with their category

SELECT p.name AS product,
       c.name AS category,
       printf('%.2f', p.price) AS price,
       p.stock
FROM products AS p
JOIN categories AS c ON c.id = p.category_id
WHERE p.discontinued = 0
ORDER BY c.name, p.name;
