-- codingpad: connection=codingpad-sample

-- Best customers

SELECT c.name AS customer,
       c.country,
       COUNT(DISTINCT o.id) AS orders,
       printf('%.2f', SUM(i.quantity * i.unit_price)) AS spent
FROM customers AS c
JOIN orders AS o ON o.customer_id = c.id
JOIN order_items AS i ON i.order_id = o.id
WHERE o.status <> 'cancelled'
GROUP BY c.id, c.name, c.country
ORDER BY SUM(i.quantity * i.unit_price) DESC, c.name;
