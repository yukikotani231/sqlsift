// Example presets for the playground. Each one is a schema, a query and a dialect.

const SHOP_SCHEMA = `CREATE TYPE order_status AS ENUM ('pending', 'paid', 'shipped', 'cancelled');

CREATE TABLE users (
    id         SERIAL PRIMARY KEY,
    email      VARCHAR(255) NOT NULL UNIQUE,
    name       TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT now()
);

CREATE TABLE orders (
    id          SERIAL PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users(id),
    status      order_status NOT NULL DEFAULT 'pending',
    total_cents INTEGER NOT NULL,
    placed_at   TIMESTAMP NOT NULL DEFAULT now()
);

CREATE TABLE order_items (
    order_id   INTEGER NOT NULL REFERENCES orders(id),
    sku        TEXT NOT NULL,
    quantity   INTEGER NOT NULL CHECK (quantity > 0),
    unit_cents INTEGER NOT NULL,
    PRIMARY KEY (order_id, sku)
);
`;

export const EXAMPLES = [
  {
    id: 'overview',
    name: 'Overview: typo + type mismatches',
    dialect: 'postgresql',
    schema: SHOP_SCHEMA,
    query: `-- Top customers by revenue
SELECT u.id, u.naem, SUM(o.total_cents) AS revenue
FROM users u
JOIN orders o ON o.user_id = u.email
WHERE o.total_cents > 'a lot'
GROUP BY u.id
ORDER BY revenue DESC
LIMIT 10;
`,
  },
  {
    id: 'typos',
    name: 'Typos in tables & columns',
    dialect: 'postgresql',
    schema: SHOP_SCHEMA,
    query: `SELECT u.emial FROM users u;

SELECT o.id, o.stauts
FROM orders o
WHERE o.user_id = 42;

SELECT COUNT(*) FROM order_itmes;
`,
  },
  {
    id: 'types',
    name: 'Type mismatches',
    dialect: 'postgresql',
    schema: SHOP_SCHEMA,
    query: `-- Comparing an INTEGER column with text
SELECT * FROM orders WHERE total_cents = 'free';

-- ...or with a boolean
SELECT * FROM orders WHERE user_id = true;

-- JOIN on columns of incompatible types
SELECT u.name
FROM users u
JOIN orders o ON o.placed_at = u.id;

-- Assigning text to an INTEGER column
UPDATE orders SET total_cents = 'ten dollars' WHERE id = 1;
`,
  },
  {
    id: 'insert',
    name: 'INSERT column count',
    dialect: 'postgresql',
    schema: SHOP_SCHEMA,
    query: `-- Two columns, one value
INSERT INTO users (email, name) VALUES ('ada@example.com');

-- Two columns, three values
INSERT INTO orders (user_id, total_cents) VALUES (1, 1999, 500);

-- This one is fine
INSERT INTO order_items (order_id, sku, quantity, unit_cents)
VALUES (1, 'SKU-001', 2, 499);
`,
  },
  {
    id: 'valid',
    name: 'CTEs & subqueries (valid)',
    dialect: 'postgresql',
    schema: SHOP_SCHEMA,
    query: `WITH monthly AS (
    SELECT user_id,
           date_trunc('month', placed_at) AS month,
           SUM(total_cents) AS cents
    FROM orders
    WHERE status <> 'cancelled'
    GROUP BY user_id, date_trunc('month', placed_at)
)
SELECT u.name, m.month, m.cents,
       RANK() OVER (PARTITION BY m.month ORDER BY m.cents DESC) AS rank
FROM monthly m
JOIN users u ON u.id = m.user_id
WHERE EXISTS (
    SELECT 1
    FROM order_items oi
    JOIN orders o ON o.id = oi.order_id
    WHERE o.user_id = u.id AND oi.quantity > 1
)
ORDER BY m.month, rank;
`,
  },
  {
    id: 'suppress',
    name: 'Inline suppression',
    dialect: 'postgresql',
    schema: SHOP_SCHEMA,
    query: `-- A column added by a migration sqlsift has not seen yet:
SELECT id, loyalty_tier FROM users; -- sqlsift:disable E0002

-- A standalone directive applies to the next line
-- sqlsift:disable
SELECT nickname FROM users;

-- Not suppressed
SELECT nickname FROM users;
`,
  },
  {
    id: 'mysql',
    name: 'MySQL dialect',
    dialect: 'mysql',
    schema: `CREATE TABLE products (
  id          INT AUTO_INCREMENT PRIMARY KEY,
  sku         VARCHAR(32) NOT NULL UNIQUE,
  title       VARCHAR(200) NOT NULL,
  price_cents INT UNSIGNED NOT NULL,
  status      ENUM('draft', 'active', 'archived') NOT NULL DEFAULT 'draft'
) ENGINE=InnoDB;

CREATE TABLE reviews (
  id         INT AUTO_INCREMENT PRIMARY KEY,
  product_id INT NOT NULL,
  rating     TINYINT NOT NULL,
  body       TEXT,
  FOREIGN KEY (product_id) REFERENCES products(id)
);
`,
    query: `SELECT p.\`titel\`, AVG(r.ratting) AS avg_rating
FROM products p
LEFT JOIN reviews r ON r.product_id = p.sku
WHERE p.status = 'active'
GROUP BY p.id
LIMIT 10;
`,
  },
  {
    id: 'sqlite',
    name: 'SQLite dialect',
    dialect: 'sqlite',
    schema: `CREATE TABLE notes (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  title      TEXT NOT NULL,
  body       TEXT,
  pinned     INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE tags (
  note_id INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
  tag     TEXT NOT NULL,
  PRIMARY KEY (note_id, tag)
);
`,
    query: `SELECT n.id, n.title, group_concat(t.tag, ', ') AS tags
FROM notes n
LEFT JOIN tags t ON t.note_id = n.id
WHERE n.pined = 1
GROUP BY n.id
ORDER BY n.created_at DESC;
`,
  },
];
