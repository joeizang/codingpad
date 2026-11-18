-- codingpad: connection=codingpad-sample

-- Revenue by category

SELECT c.name AS category,
       SUM(i.quantity) AS units,
       printf('%.2f', SUM(i.quantity * i.unit_price)) AS revenue
FROM categories AS c
JOIN products AS p ON p.category_id = c.id
JOIN order_items AS i ON i.product_id = p.id
JOIN orders AS o ON o.id = i.order_id
WHERE o.status <> 'cancelled'
GROUP BY c.id, c.name
ORDER BY SUM(i.quantity * i.unit_price) DESC;
