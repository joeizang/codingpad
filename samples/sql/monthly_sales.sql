-- codingpad: connection=sample

-- Monthly sales

SELECT strftime('%Y-%m', o.ordered_at) AS month,
       COUNT(DISTINCT o.id) AS orders,
       SUM(i.quantity) AS units,
       printf('%.2f', SUM(i.quantity * i.unit_price)) AS revenue
FROM orders AS o
JOIN order_items AS i ON i.order_id = o.id
WHERE o.status <> 'cancelled'
GROUP BY strftime('%Y-%m', o.ordered_at)
ORDER BY month;
