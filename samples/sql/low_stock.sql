-- codingpad: connection=codingpad-sample

-- Low stock and discontinued products

SELECT p.name AS product,
       c.name AS category,
       p.stock,
       CASE WHEN p.discontinued = 1 THEN 'yes' ELSE 'no' END AS discontinued
FROM products AS p
JOIN categories AS c ON c.id = p.category_id
WHERE p.stock < 20 OR p.discontinued = 1
ORDER BY p.stock, p.name;
