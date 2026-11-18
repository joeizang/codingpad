-- codingpad: connection=codingpad-sample

-- Orders and their totals

SELECT o.id,
       c.name AS customer,
       o.ordered_at,
       o.status,
       printf('%.2f', SUM(i.quantity * i.unit_price)) AS total
FROM orders AS o
JOIN customers AS c ON c.id = o.customer_id
JOIN order_items AS i ON i.order_id = o.id
GROUP BY o.id, c.name, o.ordered_at, o.status
ORDER BY o.ordered_at;
