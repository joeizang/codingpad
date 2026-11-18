-- codingpad: connection=codingpad-sample

-- Line items for every order

SELECT o.id AS order_id,
       c.name AS customer,
       o.ordered_at,
       p.name AS product,
       i.quantity,
       printf('%.2f', i.unit_price) AS unit_price,
       printf('%.2f', i.quantity * i.unit_price) AS line_total
FROM order_items AS i
JOIN orders AS o ON o.id = i.order_id
JOIN customers AS c ON c.id = o.customer_id
JOIN products AS p ON p.id = i.product_id
ORDER BY o.id, i.id;
