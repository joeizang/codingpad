-- codingpad: connection=codingpad-sample

-- Explore the bundled database

SELECT name AS object, type
FROM sqlite_master
WHERE type IN ('table', 'view')
ORDER BY name;
