//! Coverage tests for the MySQL and SQLite dialects.
//!
//! Each test function covers one feature group and iterates over a table of
//! SQL strings. Failures are collected and reported together, showing the SQL
//! and the diagnostics that were (or were not) produced.

use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::error::{Diagnostic, DiagnosticKind};
use sqlsift_core::schema::{Catalog, QualifiedName, SchemaBuilder};
use sqlsift_core::types::SqlType;

// =====================================================================
// Helpers
// =====================================================================

fn build_catalog(dialect: SqlDialect, ddl: &str) -> Catalog {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    if let Err(diags) = builder.parse(ddl) {
        panic!("schema should build without errors for {dialect}: {diags:#?}");
    }
    let (catalog, _) = builder.build();
    catalog
}

fn fmt_diags(diags: &[Diagnostic]) -> String {
    diags
        .iter()
        .map(|d| {
            format!(
                "  {} {:?} at {:?}: {} (help: {:?})",
                d.code(),
                d.kind,
                d.span.map(|s| (s.line, s.column)),
                d.message,
                d.help
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Assert that every query analyzes without any diagnostics.
fn assert_all_clean(catalog: &Catalog, dialect: SqlDialect, cases: &[&str]) {
    let mut analyzer = Analyzer::with_dialect(catalog, dialect);
    let mut failures = Vec::new();
    for sql in cases {
        let diags = analyzer.analyze(sql);
        if !diags.is_empty() {
            failures.push(format!("SQL: {sql}\n{}", fmt_diags(&diags)));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} {dialect} queries produced unexpected diagnostics:\n\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
}

/// An invalid query and the single diagnostic it is expected to produce.
struct Bad {
    sql: &'static str,
    kind: DiagnosticKind,
    /// Substring that must appear in the message.
    message: &'static str,
    /// Substring that must appear in the help text (if any).
    help: Option<&'static str>,
    /// Expected (line, column) of the primary span (if checked).
    span: Option<(usize, usize)>,
}

const fn bad(
    sql: &'static str,
    kind: DiagnosticKind,
    message: &'static str,
    help: Option<&'static str>,
    span: Option<(usize, usize)>,
) -> Bad {
    Bad {
        sql,
        kind,
        message,
        help,
        span,
    }
}

/// Assert that every query produces exactly one diagnostic matching the expectation.
fn assert_all_bad(catalog: &Catalog, dialect: SqlDialect, cases: &[Bad]) {
    let mut analyzer = Analyzer::with_dialect(catalog, dialect);
    let mut failures = Vec::new();
    for case in cases {
        let diags = analyzer.analyze(case.sql);
        let mut problems = Vec::new();
        if diags.len() != 1 {
            problems.push(format!(
                "expected exactly 1 diagnostic, got {}",
                diags.len()
            ));
        }
        if let Some(d) = diags.first() {
            if d.kind != case.kind {
                problems.push(format!("expected kind {:?}, got {:?}", case.kind, d.kind));
            }
            if !d.message.contains(case.message) {
                problems.push(format!("message should contain {:?}", case.message));
            }
            if let Some(help) = case.help {
                if !d.help.as_deref().unwrap_or("").contains(help) {
                    problems.push(format!("help should contain {help:?}"));
                }
            }
            if let Some((line, column)) = case.span {
                match d.span {
                    Some(s) if s.line == line && s.column == column => {}
                    other => problems.push(format!(
                        "expected span at ({line}, {column}), got {:?}",
                        other.map(|s| (s.line, s.column))
                    )),
                }
            }
        }
        if !problems.is_empty() {
            failures.push(format!(
                "SQL: {}\n  problems: {}\n{}",
                case.sql,
                problems.join("; "),
                fmt_diags(&diags)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} invalid {dialect} queries did not produce the expected diagnostic:\n\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
}

fn table<'a>(catalog: &'a Catalog, name: &str) -> &'a sqlsift_core::schema::TableDef {
    catalog
        .get_table(&QualifiedName::new(name))
        .unwrap_or_else(|| panic!("table `{name}` should exist in catalog"))
}

fn col_type(catalog: &Catalog, table_name: &str, column: &str) -> SqlType {
    table(catalog, table_name)
        .get_column(column)
        .unwrap_or_else(|| panic!("column `{table_name}.{column}` should exist"))
        .data_type
        .clone()
}

// =====================================================================
// MySQL schema (mysqldump style)
// =====================================================================

const MYSQL_SCHEMA: &str = r"
-- MySQL dump 10.13  Distrib 8.0.36, for Linux (x86_64)
--
-- Host: localhost    Database: shop
-- ------------------------------------------------------
-- Server version	8.0.36

/*!40101 SET @OLD_CHARACTER_SET_CLIENT=@@CHARACTER_SET_CLIENT */;
/*!40101 SET @OLD_CHARACTER_SET_RESULTS=@@CHARACTER_SET_RESULTS */;
/*!40101 SET @OLD_COLLATION_CONNECTION=@@COLLATION_CONNECTION */;
/*!50503 SET NAMES utf8mb4 */;
/*!40103 SET @OLD_TIME_ZONE=@@TIME_ZONE */;
/*!40103 SET TIME_ZONE='+00:00' */;
/*!40014 SET @OLD_UNIQUE_CHECKS=@@UNIQUE_CHECKS, UNIQUE_CHECKS=0 */;
/*!40014 SET @OLD_FOREIGN_KEY_CHECKS=@@FOREIGN_KEY_CHECKS, FOREIGN_KEY_CHECKS=0 */;
SET NAMES utf8mb4;

--
-- Table structure for table `customers`
--

DROP TABLE IF EXISTS `customers`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!50503 SET character_set_client = utf8mb4 */;
CREATE TABLE `customers` (
  `id` int unsigned NOT NULL AUTO_INCREMENT,
  `email` varchar(255) NOT NULL,
  `full_name` varchar(120) NOT NULL COMMENT 'Display name',
  `status` enum('active','suspended','deleted') NOT NULL DEFAULT 'active',
  `is_verified` tinyint(1) NOT NULL DEFAULT '0',
  `tags` set('vip','wholesale','newsletter') DEFAULT NULL,
  `balance` decimal(12,2) NOT NULL DEFAULT '0.00',
  `profile` json DEFAULT NULL,
  `birth_date` date DEFAULT NULL,
  `created_at` datetime NOT NULL DEFAULT CURRENT_TIMESTAMP,
  `updated_at` timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uk_customers_email` (`email`),
  KEY `idx_customers_status` (`status`,`created_at`)
) ENGINE=InnoDB AUTO_INCREMENT=1001 DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci COMMENT='Registered customers';
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `customers`
--

LOCK TABLES `customers` WRITE;
/*!40000 ALTER TABLE `customers` DISABLE KEYS */;
/*!40000 ALTER TABLE `customers` ENABLE KEYS */;
UNLOCK TABLES;

DROP TABLE IF EXISTS `products`;
CREATE TABLE `products` (
  `id` bigint unsigned NOT NULL AUTO_INCREMENT,
  `sku` char(12) NOT NULL,
  `name` varchar(200) NOT NULL,
  `description` mediumtext,
  `long_description` longtext,
  `summary` tinytext,
  `price` decimal(10,2) NOT NULL,
  `cost` double DEFAULT NULL,
  `weight_kg` float DEFAULT NULL,
  `stock` int NOT NULL DEFAULT '0',
  `is_active` tinyint(1) NOT NULL DEFAULT '1',
  `attributes` json DEFAULT NULL,
  `price_with_tax` decimal(10,2) GENERATED ALWAYS AS ((`price` * 1.2)) STORED,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`id`),
  UNIQUE KEY `uk_products_sku` (`sku`),
  KEY `idx_products_name` (`name`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

DROP TABLE IF EXISTS `orders`;
CREATE TABLE `orders` (
  `id` bigint unsigned NOT NULL AUTO_INCREMENT,
  `customer_id` int unsigned NOT NULL,
  `status` enum('pending','paid','shipped','cancelled') NOT NULL DEFAULT 'pending',
  `total` decimal(12,2) NOT NULL DEFAULT '0.00',
  `coupon_code` varchar(32) DEFAULT NULL,
  `placed_at` datetime NOT NULL,
  `shipped_at` datetime DEFAULT NULL,
  `notes` text,
  PRIMARY KEY (`id`),
  KEY `fk_orders_customer` (`customer_id`),
  CONSTRAINT `fk_orders_customer` FOREIGN KEY (`customer_id`) REFERENCES `customers` (`id`) ON DELETE CASCADE ON UPDATE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

DROP TABLE IF EXISTS `order_items`;
CREATE TABLE `order_items` (
  `order_id` bigint unsigned NOT NULL,
  `product_id` bigint unsigned NOT NULL,
  `quantity` smallint unsigned NOT NULL DEFAULT '1',
  `unit_price` decimal(10,2) NOT NULL,
  PRIMARY KEY (`order_id`,`product_id`),
  KEY `fk_items_product` (`product_id`),
  CONSTRAINT `fk_items_order` FOREIGN KEY (`order_id`) REFERENCES `orders` (`id`),
  CONSTRAINT `fk_items_product` FOREIGN KEY (`product_id`) REFERENCES `products` (`id`)
) ENGINE=InnoDB;

DROP TABLE IF EXISTS `settings`;
CREATE TABLE `settings` (
  `key` varchar(64) NOT NULL,
  `value` text NOT NULL,
  `updated_by` int unsigned DEFAULT NULL,
  PRIMARY KEY (`key`)
) ENGINE=MyISAM DEFAULT CHARSET=latin1;

CREATE TABLE IF NOT EXISTS `audit_log` (
  `id` bigint NOT NULL AUTO_INCREMENT,
  `event_time` timestamp(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
  `actor` varchar(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `action` varchar(32) NOT NULL,
  `payload` json NOT NULL,
  `ip` varbinary(16) DEFAULT NULL,
  PRIMARY KEY (`id`),
  INDEX `idx_audit_time` (`event_time`)
) ENGINE=InnoDB;

/*!40103 SET TIME_ZONE=@OLD_TIME_ZONE */;
/*!40014 SET FOREIGN_KEY_CHECKS=@OLD_FOREIGN_KEY_CHECKS */;
/*!40014 SET UNIQUE_CHECKS=@OLD_UNIQUE_CHECKS */;
-- Dump completed on 2024-05-01 12:00:00
";

fn mysql_catalog() -> Catalog {
    build_catalog(SqlDialect::MySQL, MYSQL_SCHEMA)
}

fn mysql_clean(cases: &[&str]) {
    assert_all_clean(&mysql_catalog(), SqlDialect::MySQL, cases);
}

fn mysql_bad(cases: &[Bad]) {
    assert_all_bad(&mysql_catalog(), SqlDialect::MySQL, cases);
}

#[test]
fn mysql_schema_dump_builds_all_tables() {
    let catalog = mysql_catalog();
    for name in [
        "customers",
        "products",
        "orders",
        "order_items",
        "settings",
        "audit_log",
    ] {
        assert!(
            catalog.table_exists(&QualifiedName::new(name)),
            "table `{name}` should exist; tables = {:?}",
            catalog.table_names()
        );
    }
    assert_eq!(table(&catalog, "customers").columns.len(), 11);
    assert_eq!(table(&catalog, "products").columns.len(), 14);
    assert_eq!(table(&catalog, "orders").columns.len(), 8);
    assert_eq!(table(&catalog, "order_items").columns.len(), 4);
    assert_eq!(table(&catalog, "settings").columns.len(), 3);
    assert_eq!(table(&catalog, "audit_log").columns.len(), 6);
}

#[test]
fn mysql_schema_column_types() {
    let catalog = mysql_catalog();
    let cases: &[(&str, &str, SqlType)] = &[
        ("customers", "id", SqlType::Integer),
        ("customers", "email", SqlType::Varchar { length: Some(255) }),
        ("customers", "is_verified", SqlType::TinyInt),
        (
            "customers",
            "status",
            SqlType::Enum(vec!["active".into(), "suspended".into(), "deleted".into()]),
        ),
        (
            "customers",
            "balance",
            SqlType::Decimal {
                precision: Some(12),
                scale: Some(2),
            },
        ),
        ("customers", "profile", SqlType::Json),
        ("customers", "birth_date", SqlType::Date),
        (
            "customers",
            "created_at",
            SqlType::Timestamp {
                precision: None,
                with_timezone: false,
            },
        ),
        (
            "customers",
            "updated_at",
            SqlType::Timestamp {
                precision: None,
                with_timezone: false,
            },
        ),
        ("products", "id", SqlType::BigInt),
        ("products", "sku", SqlType::Char { length: Some(12) }),
        ("products", "cost", SqlType::DoublePrecision),
        ("products", "stock", SqlType::Integer),
        ("orders", "notes", SqlType::Text),
        ("order_items", "quantity", SqlType::SmallInt),
        ("audit_log", "ip", SqlType::Bytea),
    ];
    let mut failures = Vec::new();
    for (t, c, expected) in cases {
        let actual = col_type(&catalog, t, c);
        if &actual != expected {
            failures.push(format!("{t}.{c}: expected {expected:?}, got {actual:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn mysql_schema_constraints_and_nullability() {
    let catalog = mysql_catalog();
    let customers = table(&catalog, "customers");
    let id = customers.get_column("id").unwrap();
    assert!(!id.nullable, "AUTO_INCREMENT NOT NULL id");
    assert!(
        customers
            .primary_key
            .as_ref()
            .is_some_and(|pk| pk.columns == vec!["id".to_string()]),
        "table-level PRIMARY KEY (`id`): {:?}",
        customers.primary_key
    );
    assert!(customers.get_column("email").map(|c| !c.nullable).unwrap());
    assert!(customers.get_column("tags").map(|c| c.nullable).unwrap());
    assert!(customers.get_column("profile").map(|c| c.nullable).unwrap());

    let items = table(&catalog, "order_items");
    assert!(
        items
            .primary_key
            .as_ref()
            .is_some_and(|pk| pk.columns == vec!["order_id", "product_id"]),
        "composite PK: {:?}",
        items.primary_key
    );
    assert_eq!(
        items.foreign_keys.len(),
        2,
        "two named FKs: {:?}",
        items.foreign_keys
    );
    let orders = table(&catalog, "orders");
    assert_eq!(orders.foreign_keys.len(), 1);
    assert_eq!(orders.foreign_keys[0].references_table.name, "customers");
}

// =====================================================================
// MySQL valid queries
// =====================================================================

#[test]
fn mysql_valid_identifiers_and_placeholders() {
    mysql_clean(&[
        "SELECT `id`, `email` FROM `customers`",
        "SELECT `c`.`id`, `c`.`full_name` FROM `customers` AS `c`",
        "SELECT `key`, `value` FROM `settings` WHERE `key` = ?",
        "SELECT s.`key` FROM settings s WHERE s.`value` LIKE ?",
        "SELECT ID, Email, FULL_NAME FROM customers",
        "SELECT c.ID FROM customers c WHERE c.Status = 'active'",
        "SELECT id FROM customers WHERE id = ? AND status = ?",
        "SELECT id FROM orders WHERE customer_id IN (?, ?, ?)",
        "SELECT id FROM orders WHERE total BETWEEN ? AND ?",
        "INSERT INTO customers (email, full_name, status) VALUES (?, ?, ?)",
        "UPDATE customers SET full_name = ?, balance = ? WHERE id = ?",
        "DELETE FROM orders WHERE id = ? LIMIT 1",
        "SELECT id FROM customers WHERE email = \"a@example.com\"",
    ]);
}

#[test]
fn mysql_valid_limit_and_ordering() {
    mysql_clean(&[
        "SELECT id FROM customers ORDER BY created_at DESC LIMIT 10",
        "SELECT id FROM customers ORDER BY id LIMIT 20, 10",
        "SELECT id FROM customers ORDER BY id LIMIT 10 OFFSET 20",
        "SELECT id FROM customers LIMIT ?, ?",
        "SELECT full_name AS n FROM customers ORDER BY n",
        "SELECT status, COUNT(*) AS cnt FROM customers GROUP BY status ORDER BY cnt DESC",
        "SELECT DISTINCT status FROM orders",
        "SELECT id FROM products ORDER BY FIELD(id, 3, 1, 2)",
    ]);
}

#[test]
fn mysql_valid_insert_variants() {
    mysql_clean(&[
        "INSERT INTO customers (email, full_name) VALUES ('a@x.io', 'A'), ('b@x.io', 'B')",
        "INSERT IGNORE INTO customers (email, full_name) VALUES ('a@x.io', 'A')",
        "REPLACE INTO settings (`key`, `value`) VALUES ('theme', 'dark')",
        "INSERT INTO settings (`key`, `value`) VALUES ('theme', 'dark') \
         ON DUPLICATE KEY UPDATE `value` = VALUES(`value`)",
        "INSERT INTO order_items (order_id, product_id, quantity, unit_price) VALUES (1, 2, 3, 9.99) \
         ON DUPLICATE KEY UPDATE quantity = quantity + VALUES(quantity)",
        "INSERT INTO customers (email, full_name, status) VALUES ('d@x.io', 'D', 'suspended')",
        "INSERT INTO customers (email, full_name, balance) VALUES ('e@x.io', 'E', 12.50)",
        "INSERT INTO customers (email, full_name, is_verified) VALUES ('f@x.io', 'F', 1)",
        "INSERT INTO customers (email, full_name, profile) VALUES ('g@x.io', 'G', JSON_OBJECT('a', 1))",
        "INSERT INTO audit_log (actor, action, payload) SELECT full_name, 'export', JSON_OBJECT('id', id) FROM customers",
        "INSERT INTO products (sku, name, price) VALUES ('SKU000000001', 'Widget', 10)",
        "INSERT INTO `settings` (`key`, `value`, `updated_by`) VALUES ('a', 'b', NULL)",
    ]);
}

#[test]
fn mysql_valid_multi_table_update_delete() {
    mysql_clean(&[
        "UPDATE orders o JOIN customers c ON c.id = o.customer_id SET o.status = 'cancelled' WHERE c.status = 'deleted'",
        "UPDATE orders o INNER JOIN order_items oi ON oi.order_id = o.id SET o.total = o.total + oi.unit_price WHERE oi.quantity > 1",
        "DELETE o FROM orders o JOIN customers c ON c.id = o.customer_id WHERE c.status = 'deleted'",
        "DELETE oi FROM order_items oi LEFT JOIN products p ON p.id = oi.product_id WHERE p.id IS NULL",
        "UPDATE products SET stock = stock - 1 WHERE id = 5 AND stock > 0",
        "DELETE FROM audit_log WHERE event_time < NOW() - INTERVAL 90 DAY",
        "DELETE FROM audit_log ORDER BY id LIMIT 100",
    ]);
}

#[test]
fn mysql_valid_builtin_functions() {
    mysql_clean(&[
        "SELECT IFNULL(coupon_code, 'none') FROM orders",
        "SELECT IF(is_verified = 1, 'yes', 'no') AS verified FROM customers",
        "SELECT CONCAT(full_name, ' <', email, '>') FROM customers",
        "SELECT CONCAT_WS(',', full_name, email) FROM customers",
        "SELECT id FROM orders WHERE placed_at >= CURDATE()",
        "SELECT id FROM orders WHERE DATE(placed_at) = CURDATE()",
        "SELECT id FROM orders WHERE placed_at > DATE_SUB(NOW(), INTERVAL 7 DAY)",
        "SELECT DATE_ADD(placed_at, INTERVAL 1 DAY) FROM orders",
        "SELECT DATEDIFF(shipped_at, placed_at) AS days FROM orders",
        "SELECT UNIX_TIMESTAMP(created_at) FROM customers",
        "SELECT id FROM customers WHERE YEAR(created_at) = 2024",
        "SELECT LPAD(id, 8, '0') FROM customers",
        "SELECT SUBSTRING_INDEX(email, '@', -1) AS domain FROM customers",
        "SELECT ROUND(price * 1.1, 2) FROM products",
        "SELECT FORMAT(balance, 2) FROM customers",
        "SELECT UUID(), LAST_INSERT_ID()",
        "SELECT GREATEST(price, 1), LEAST(stock, 100) FROM products",
        "SELECT COALESCE(cost, price) FROM products",
        "SELECT CAST(price AS CHAR) FROM products",
        "SELECT CAST(stock AS UNSIGNED) FROM products",
        "SELECT CONVERT(name USING utf8mb4) FROM products",
        "SELECT id FROM customers WHERE FIND_IN_SET('vip', tags) > 0",
    ]);
}

#[test]
fn mysql_valid_json_and_regexp() {
    mysql_clean(&[
        "SELECT profile->'$.address.city' FROM customers",
        "SELECT profile->>'$.address.city' AS city FROM customers",
        "SELECT id FROM customers WHERE profile->>'$.plan' = 'pro'",
        "SELECT JSON_EXTRACT(attributes, '$.color') FROM products",
        "SELECT JSON_UNQUOTE(JSON_EXTRACT(attributes, '$.color')) FROM products",
        "SELECT id FROM products WHERE JSON_CONTAINS(attributes, '\"red\"', '$.colors')",
        "SELECT JSON_ARRAYAGG(id) FROM products",
        "SELECT id FROM customers WHERE email REGEXP '^[a-z]+@example\\.com$'",
        "SELECT id FROM customers WHERE email NOT REGEXP 'spam'",
        "SELECT id FROM customers WHERE email RLIKE 'x'",
        "SELECT id FROM customers WHERE full_name LIKE 'A%'",
    ]);
}

#[test]
fn mysql_valid_aggregates_and_group_concat() {
    mysql_clean(&[
        "SELECT customer_id, GROUP_CONCAT(id) FROM orders GROUP BY customer_id",
        "SELECT customer_id, GROUP_CONCAT(DISTINCT status ORDER BY status SEPARATOR ', ') FROM orders GROUP BY customer_id",
        "SELECT customer_id, SUM(total), AVG(total), MAX(placed_at) FROM orders GROUP BY customer_id HAVING SUM(total) > 100",
        "SELECT COUNT(DISTINCT customer_id) FROM orders",
        "SELECT product_id, SUM(quantity * unit_price) AS revenue FROM order_items GROUP BY product_id",
    ]);
}

#[test]
fn mysql_valid_type_compatibility() {
    mysql_clean(&[
        // DECIMAL vs integer literals
        "SELECT id FROM products WHERE price > 10",
        "SELECT id FROM products WHERE price > 9.99",
        // TINYINT(1) vs integer literals
        "SELECT id FROM products WHERE is_active = 1",
        "SELECT id FROM customers WHERE is_verified <> 0",
        // ENUM vs string literal
        "SELECT id FROM orders WHERE status = 'paid'",
        "SELECT id FROM orders WHERE status IN ('paid', 'shipped')",
        "UPDATE orders SET status = 'shipped' WHERE id = 1",
        // integer widening across columns
        "SELECT oi.order_id FROM order_items oi WHERE oi.quantity < oi.order_id",
        "SELECT id FROM orders WHERE customer_id = id",
        // arithmetic between numeric columns
        "SELECT price - cost FROM products",
        "SELECT stock * price FROM products WHERE stock * price > 1000",
        // NULL comparisons
        "SELECT id FROM orders WHERE shipped_at IS NULL",
        "SELECT id FROM orders WHERE coupon_code IS NOT NULL",
        "UPDATE orders SET shipped_at = NULL WHERE id = 1",
        // JOINs on compatible types (INT UNSIGNED vs BIGINT UNSIGNED)
        "SELECT o.id FROM orders o JOIN customers c ON o.customer_id = c.id",
        "SELECT oi.order_id FROM order_items oi JOIN orders o ON o.id = oi.order_id",
        // date column vs date function
        "SELECT id FROM customers WHERE birth_date < CURDATE()",
        // string column compatibility
        "SELECT id FROM products WHERE sku = name",
    ]);
}

#[test]
fn mysql_valid_joins_subqueries_ctes_windows() {
    mysql_clean(&[
        "SELECT c.id FROM customers c LEFT JOIN orders o ON o.customer_id = c.id WHERE o.id IS NULL",
        "SELECT c.id FROM customers c RIGHT JOIN orders o ON o.customer_id = c.id",
        "SELECT c.id, p.id FROM customers c CROSS JOIN products p",
        "SELECT * FROM orders NATURAL JOIN customers",
        "SELECT id FROM customers WHERE EXISTS (SELECT 1 FROM orders o WHERE o.customer_id = customers.id)",
        "SELECT id, (SELECT COUNT(*) FROM orders o WHERE o.customer_id = c.id) AS n FROM customers c",
        "SELECT t.customer_id, t.n FROM (SELECT customer_id, COUNT(*) AS n FROM orders GROUP BY customer_id) AS t WHERE t.n > 3",
        "WITH big AS (SELECT customer_id, SUM(total) AS spent FROM orders GROUP BY customer_id) \
         SELECT c.full_name, big.spent FROM customers c JOIN big ON big.customer_id = c.id",
        "WITH RECURSIVE seq (n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 10) SELECT n FROM seq",
        "SELECT id, ROW_NUMBER() OVER (PARTITION BY customer_id ORDER BY placed_at) AS rn FROM orders",
        "SELECT id, SUM(total) OVER (ORDER BY placed_at ROWS BETWEEN 2 PRECEDING AND CURRENT ROW) FROM orders",
        "SELECT id, LAG(total) OVER w FROM orders WINDOW w AS (PARTITION BY customer_id ORDER BY id)",
        "SELECT id, DENSE_RANK() OVER (ORDER BY price DESC) FROM products",
        "SELECT id FROM customers UNION SELECT customer_id FROM orders",
        "SELECT id FROM customers c WHERE c.id = ANY (SELECT customer_id FROM orders)",
        "SELECT id FROM products FOR UPDATE",
    ]);
}

// =====================================================================
// MySQL invalid queries
// =====================================================================

#[test]
fn mysql_invalid_table_not_found() {
    use DiagnosticKind::TableNotFound;
    mysql_bad(&[
        bad(
            "SELECT COUNT(*) FROM customer",
            TableNotFound,
            "customer",
            None,
            Some((1, 22)),
        ),
        bad(
            "SELECT COUNT(*) FROM `custmers`",
            TableNotFound,
            "custmers",
            None,
            Some((1, 22)),
        ),
        bad(
            "INSERT INTO order_item (order_id) VALUES (1)",
            TableNotFound,
            "order_item",
            Some("Did you mean 'order_items'?"),
            Some((1, 13)),
        ),
        bad(
            "UPDATE setting SET `value` = 'x'",
            TableNotFound,
            "setting",
            None,
            None,
        ),
        bad(
            "DELETE FROM audit_logs",
            TableNotFound,
            "audit_logs",
            None,
            None,
        ),
        bad(
            "SELECT c.id FROM customers c\nCROSS JOIN ordrs",
            TableNotFound,
            "ordrs",
            None,
            Some((2, 12)),
        ),
    ]);
}

#[test]
fn mysql_invalid_column_not_found() {
    use DiagnosticKind::ColumnNotFound;
    mysql_bad(&[
        bad(
            "SELECT emial FROM customers",
            ColumnNotFound,
            "emial",
            Some("Did you mean 'email'?"),
            Some((1, 8)),
        ),
        bad(
            "SELECT `fullname` FROM `customers`",
            ColumnNotFound,
            "fullname",
            Some("Did you mean 'full_name'?"),
            Some((1, 8)),
        ),
        bad(
            "SELECT id FROM orders WHERE totl > 10",
            ColumnNotFound,
            "totl",
            Some("Did you mean 'total'?"),
            Some((1, 29)),
        ),
        bad(
            "SELECT c.id FROM customers c WHERE c.balanse > 0",
            ColumnNotFound,
            "balanse",
            Some("Did you mean 'balance'?"),
            None,
        ),
        bad(
            "INSERT INTO customers (email, fullname) VALUES ('a', 'b')",
            ColumnNotFound,
            "fullname",
            Some("Did you mean 'full_name'?"),
            Some((1, 31)),
        ),
        bad(
            "UPDATE products SET stok = 1 WHERE id = 1",
            ColumnNotFound,
            "stok",
            Some("Did you mean 'stock'?"),
            Some((1, 21)),
        ),
        bad(
            "DELETE FROM orders WHERE placed = NOW()",
            ColumnNotFound,
            "placed",
            None,
            None,
        ),
        bad(
            "SELECT id FROM products ORDER BY prise",
            ColumnNotFound,
            "prise",
            Some("Did you mean 'price'?"),
            None,
        ),
        bad(
            "SELECT customer_id, COUNT(*) FROM orders GROUP BY custmer_id",
            ColumnNotFound,
            "custmer_id",
            Some("Did you mean 'customer_id'?"),
            None,
        ),
        bad(
            "SELECT id,\n       created\nFROM orders",
            ColumnNotFound,
            "created",
            None,
            Some((2, 8)),
        ),
        bad(
            "SELECT o.id FROM orders o JOIN customers c ON c.id = o.customerid",
            ColumnNotFound,
            "customerid",
            Some("Did you mean 'customer_id'?"),
            None,
        ),
        bad(
            "SELECT id FROM customers WHERE id IN (SELECT customer FROM orders)",
            ColumnNotFound,
            "customer",
            None,
            None,
        ),
        bad(
            "WITH t AS (SELECT id, total FROM orders) SELECT t.totals FROM t",
            ColumnNotFound,
            "totals",
            None,
            None,
        ),
    ]);
}

#[test]
fn mysql_invalid_type_mismatch() {
    use DiagnosticKind::TypeMismatch;
    mysql_bad(&[
        bad(
            "SELECT id FROM customers WHERE email = 42",
            TypeMismatch,
            "cannot compare",
            Some("CAST"),
            Some((1, 32)),
        ),
        bad(
            "SELECT id FROM products WHERE name > price",
            TypeMismatch,
            "cannot compare",
            None,
            Some((1, 31)),
        ),
        bad(
            "SELECT id FROM customers WHERE profile = 1",
            TypeMismatch,
            "json",
            None,
            None,
        ),
        bad(
            "SELECT full_name + 1 FROM customers",
            TypeMismatch,
            "Arithmetic",
            None,
            Some((1, 8)),
        ),
        bad(
            "INSERT INTO products (sku, name, price, stock) VALUES ('S', 'N', 1, 'many')",
            TypeMismatch,
            "stock",
            Some("CAST"),
            None,
        ),
        bad(
            "UPDATE order_items SET quantity = 'three' WHERE order_id = 1",
            TypeMismatch,
            "quantity",
            None,
            None,
        ),
        bad(
            "UPDATE customers SET birth_date = 5 WHERE id = 1",
            TypeMismatch,
            "birth_date",
            None,
            None,
        ),
        bad(
            "SELECT id FROM orders WHERE customer_id = UPPER(notes)",
            TypeMismatch,
            "cannot compare",
            None,
            None,
        ),
    ]);
}

#[test]
fn mysql_invalid_join_type_mismatch() {
    use DiagnosticKind::JoinTypeMismatch;
    mysql_bad(&[
        bad(
            "SELECT o.id FROM orders o JOIN customers c ON c.email = o.customer_id",
            JoinTypeMismatch,
            "JOIN condition type mismatch",
            Some("CAST"),
            Some((1, 47)),
        ),
        bad(
            "SELECT p.id FROM products p\nJOIN settings s ON p.id = s.`key`",
            JoinTypeMismatch,
            "bigint vs varchar",
            None,
            Some((2, 20)),
        ),
        bad(
            "SELECT a.id FROM audit_log a LEFT JOIN customers c ON a.payload = c.id",
            JoinTypeMismatch,
            "json",
            None,
            None,
        ),
    ]);
}

#[test]
fn mysql_invalid_column_count_ambiguous_null() {
    use DiagnosticKind::*;
    mysql_bad(&[
        bad(
            "INSERT INTO settings (`key`, `value`) VALUES ('a')",
            ColumnCountMismatch,
            "1 value(s) but 2 column(s)",
            Some("Provide 2 value(s)"),
            None,
        ),
        bad(
            "INSERT INTO settings VALUES ('a', 'b')",
            ColumnCountMismatch,
            "2 value(s) but 3 column(s)",
            Some("has 3 columns"),
            None,
        ),
        bad(
            "INSERT INTO order_items (order_id, product_id) VALUES (1, 2, 3)",
            ColumnCountMismatch,
            "3 value(s) but 2 column(s)",
            None,
            None,
        ),
        bad(
            "SELECT id FROM orders o JOIN customers c ON c.id = o.customer_id",
            AmbiguousColumn,
            "ambiguous",
            Some("Qualify the column"),
            Some((1, 8)),
        ),
        bad(
            "SELECT o.id FROM orders o JOIN customers c ON c.id = o.customer_id WHERE status = 'paid'",
            AmbiguousColumn,
            "status",
            None,
            None,
        ),
        bad(
            "INSERT INTO customers (email, full_name) VALUES (NULL, 'x')",
            PotentialNullViolation,
            "email",
            Some("NOT NULL"),
            None,
        ),
        bad(
            "UPDATE orders SET placed_at = NULL WHERE id = 1",
            PotentialNullViolation,
            "placed_at",
            None,
            None,
        ),
    ]);
}

#[test]
fn mysql_invalid_reports_every_error() {
    let catalog = mysql_catalog();
    let mut analyzer = Analyzer::with_dialect(&catalog, SqlDialect::MySQL);
    let sql =
        "SELECT `emial` FROM customers;\nSELECT COUNT(*) FROM nope;\nUPDATE orders SET totl = 1";
    let diags = analyzer.analyze(sql);
    let kinds: Vec<_> = diags.iter().map(|d| d.kind).collect();
    assert_eq!(
        kinds,
        vec![
            DiagnosticKind::ColumnNotFound,
            DiagnosticKind::TableNotFound,
            DiagnosticKind::ColumnNotFound
        ],
        "SQL: {sql}\n{}",
        fmt_diags(&diags)
    );
    let lines: Vec<_> = diags.iter().map(|d| d.span.map(|s| s.line)).collect();
    assert_eq!(
        lines,
        vec![Some(1), Some(2), Some(3)],
        "{}",
        fmt_diags(&diags)
    );
}

// =====================================================================
// SQLite schema
// =====================================================================

const SQLITE_SCHEMA: &str = r"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
BEGIN TRANSACTION;

CREATE TABLE IF NOT EXISTS authors (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    email TEXT UNIQUE,
    bio,
    is_active BOOLEAN NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS posts (
    id INTEGER PRIMARY KEY,
    author_id INTEGER NOT NULL REFERENCES authors(id) ON DELETE CASCADE,
    title TEXT NOT NULL CHECK (length(title) > 0),
    body TEXT,
    status TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'published', 'archived')),
    view_count INTEGER NOT NULL DEFAULT 0,
    rating REAL,
    metadata TEXT,
    published_at DATETIME,
    updated_at TIMESTAMP DEFAULT (datetime('now'))
);

CREATE TABLE tags (
    id INTEGER PRIMARY KEY,
    name TEXT COLLATE NOCASE NOT NULL UNIQUE
);

CREATE TABLE post_tags (
    post_id INTEGER NOT NULL REFERENCES posts(id),
    tag_id INTEGER NOT NULL REFERENCES tags(id),
    PRIMARY KEY (post_id, tag_id)
) WITHOUT ROWID;

CREATE TABLE kv (
    key TEXT PRIMARY KEY,
    value ANY,
    expires_at INTEGER
) STRICT;

CREATE TABLE events (
    id INTEGER PRIMARY KEY,
    kind,
    payload BLOB,
    occurred_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_posts_author ON posts(author_id);
CREATE UNIQUE INDEX idx_tags_name ON tags(name);

CREATE TRIGGER IF NOT EXISTS posts_touch AFTER UPDATE ON posts
BEGIN
    UPDATE posts SET updated_at = datetime('now') WHERE id = NEW.id;
END;

CREATE VIEW published_posts AS
    SELECT id, author_id, title FROM posts WHERE status = 'published';

COMMIT;
";

fn sqlite_catalog() -> Catalog {
    build_catalog(SqlDialect::SQLite, SQLITE_SCHEMA)
}

fn sqlite_clean(cases: &[&str]) {
    assert_all_clean(&sqlite_catalog(), SqlDialect::SQLite, cases);
}

fn sqlite_bad(cases: &[Bad]) {
    assert_all_bad(&sqlite_catalog(), SqlDialect::SQLite, cases);
}

#[test]
fn sqlite_schema_builds_all_tables() {
    let catalog = sqlite_catalog();
    for name in ["authors", "posts", "tags", "post_tags", "kv", "events"] {
        assert!(
            catalog.table_exists(&QualifiedName::new(name)),
            "table `{name}` should exist; tables = {:?}",
            catalog.table_names()
        );
    }
    assert!(
        catalog.view_exists(&QualifiedName::new("published_posts")),
        "view should exist"
    );
    assert_eq!(table(&catalog, "authors").columns.len(), 6);
    assert_eq!(table(&catalog, "posts").columns.len(), 10);
    assert_eq!(table(&catalog, "post_tags").columns.len(), 2);
    assert_eq!(table(&catalog, "kv").columns.len(), 3);
    assert_eq!(table(&catalog, "events").columns.len(), 4);
}

#[test]
fn sqlite_schema_column_types_and_constraints() {
    let catalog = sqlite_catalog();
    assert_eq!(col_type(&catalog, "authors", "id"), SqlType::Integer);
    assert_eq!(col_type(&catalog, "authors", "name"), SqlType::Text);
    assert_eq!(col_type(&catalog, "authors", "is_active"), SqlType::Boolean);
    assert_eq!(col_type(&catalog, "authors", "bio"), SqlType::Unknown);
    assert_eq!(col_type(&catalog, "events", "kind"), SqlType::Unknown);
    assert_eq!(col_type(&catalog, "events", "payload"), SqlType::Bytea);
    assert_eq!(col_type(&catalog, "posts", "rating"), SqlType::Real);

    let authors = table(&catalog, "authors");
    let id = authors.get_column("id").unwrap();
    assert!(id.is_primary_key && !id.nullable);
    assert!(authors.get_column("bio").unwrap().nullable);

    let posts = table(&catalog, "posts");
    assert!(posts.get_column("id").unwrap().is_primary_key);
    assert!(!posts.get_column("author_id").unwrap().nullable);
    assert_eq!(
        posts.check_constraints.len(),
        2,
        "column-level CHECK constraints should be recorded: {:?}",
        posts.check_constraints
    );

    let post_tags = table(&catalog, "post_tags");
    assert!(post_tags
        .primary_key
        .as_ref()
        .is_some_and(|pk| pk.columns == vec!["post_id", "tag_id"]));
}

// =====================================================================
// SQLite valid queries
// =====================================================================

#[test]
fn sqlite_valid_parameters() {
    sqlite_clean(&[
        "SELECT id, name FROM authors WHERE id = ?",
        "SELECT id FROM posts WHERE author_id = ?1 AND status = ?2",
        "SELECT id FROM posts WHERE author_id = :author_id",
        "SELECT id FROM posts WHERE author_id = @author",
        "SELECT id FROM posts WHERE author_id = $author",
        "INSERT INTO authors (name, email) VALUES (?, ?)",
        "INSERT INTO authors (name, email) VALUES (:name, :email)",
        "UPDATE posts SET title = ?1, body = ?2 WHERE id = ?3",
        "DELETE FROM posts WHERE id = :id",
        "SELECT id FROM posts LIMIT ? OFFSET ?",
    ]);
}

#[test]
fn sqlite_valid_upsert_and_returning() {
    sqlite_clean(&[
        "INSERT OR IGNORE INTO tags (name) VALUES ('rust')",
        "INSERT OR REPLACE INTO kv (key, expires_at) VALUES ('a', 1)",
        "REPLACE INTO kv (key, expires_at) VALUES ('a', 1)",
        "INSERT INTO kv (key, expires_at) VALUES ('a', 1) ON CONFLICT (key) DO NOTHING",
        "INSERT INTO kv (key, expires_at) VALUES ('a', 1) ON CONFLICT DO NOTHING",
        "INSERT INTO kv (key, expires_at) VALUES ('a', 1) ON CONFLICT (key) DO UPDATE SET expires_at = excluded.expires_at",
        "INSERT INTO kv (key, expires_at) VALUES ('a', 1) \
         ON CONFLICT (key) DO UPDATE SET expires_at = excluded.expires_at WHERE kv.expires_at < excluded.expires_at",
        "INSERT INTO tags (name) VALUES ('rust') ON CONFLICT (name) DO UPDATE SET name = excluded.name RETURNING id",
        "INSERT INTO authors (name) VALUES ('x') RETURNING id",
        "INSERT INTO authors (name) VALUES ('x') RETURNING id, name, created_at",
        "UPDATE posts SET view_count = view_count + 1 WHERE id = 1 RETURNING view_count",
        "DELETE FROM posts WHERE status = 'archived' RETURNING id",
        "INSERT INTO post_tags (post_id, tag_id) SELECT p.id, t.id FROM posts p, tags t WHERE t.name = 'rust'",
        // (DEFAULT VALUES would fail: authors.name is NOT NULL without a default)
        "INSERT INTO authors (name) VALUES ('anon')",
        "UPDATE OR IGNORE tags SET name = 'x' WHERE id = 1",
    ]);
}

#[test]
fn sqlite_valid_builtin_functions() {
    sqlite_clean(&[
        "SELECT datetime('now')",
        "SELECT id FROM posts WHERE published_at > datetime('now', '-7 days')",
        "SELECT date(created_at), time(created_at), julianday(created_at) FROM authors",
        "SELECT unixepoch(created_at) FROM authors",
        "SELECT json_extract(metadata, '$.lang') FROM posts",
        "SELECT id FROM posts WHERE json_extract(metadata, '$.featured') = 1",
        "SELECT metadata -> '$.lang', metadata ->> '$.lang' FROM posts",
        "SELECT json_object('id', id, 'title', title) FROM posts",
        "SELECT json_group_array(name) FROM tags",
        "SELECT name || ' <' || email || '>' FROM authors",
        "SELECT IIF(view_count > 100, 'hot', 'cold') FROM posts",
        "SELECT ifnull(body, ''), coalesce(rating, 0.0) FROM posts",
        "SELECT typeof(bio), typeof(id) FROM authors",
        "SELECT substr(title, 1, 10), instr(title, 'a'), replace(title, 'a', 'b') FROM posts",
        "SELECT printf('%05d', id) FROM posts",
        "SELECT hex(payload), length(payload) FROM events",
        "SELECT group_concat(name, ',') FROM tags",
        "SELECT total(view_count), avg(rating) FROM posts",
        "SELECT max(id), min(id) FROM posts",
        "SELECT random(), abs(-1), round(rating, 1) FROM posts",
        "SELECT last_insert_rowid(), changes()",
        "SELECT CAST(view_count AS TEXT) FROM posts",
        "SELECT quote(name), lower(name), upper(name), trim(name) FROM authors",
        "SELECT nullif(body, '') FROM posts",
    ]);
}

#[test]
fn sqlite_valid_operators_and_types() {
    sqlite_clean(&[
        "SELECT id FROM posts ORDER BY id LIMIT 10 OFFSET 20",
        "SELECT id FROM posts LIMIT 5",
        "SELECT id FROM tags WHERE name LIKE 'r%' ESCAPE '\\'",
        "SELECT id FROM posts WHERE rating > 4",
        "SELECT id FROM posts WHERE rating > 4.5",
        "SELECT id FROM posts WHERE view_count BETWEEN 10 AND 100",
        "SELECT id FROM authors WHERE is_active = TRUE",
        "SELECT id FROM authors WHERE is_active",
        "SELECT id FROM authors WHERE NOT is_active",
        "SELECT id FROM posts WHERE body IS NULL",
        "SELECT id FROM posts WHERE body IS NOT NULL AND body <> ''",
        "SELECT id FROM posts WHERE status IN ('draft', 'published')",
        "SELECT id FROM posts WHERE rating IS NOT NULL",
        "SELECT id, view_count % 2 FROM posts",
        "SELECT id FROM posts WHERE view_count * 2 > author_id",
        "SELECT id FROM posts WHERE title COLLATE NOCASE = 'hello'",
        "SELECT CASE WHEN rating >= 4 THEN 'good' ELSE 'bad' END FROM posts",
    ]);
}

#[test]
fn sqlite_valid_case_insensitive_columns() {
    sqlite_clean(&[
        "SELECT ID, Name FROM authors",
        "SELECT p.TITLE, p.Author_Id FROM posts p",
        "UPDATE posts SET VIEW_COUNT = 0 WHERE ID = 1",
        "INSERT INTO tags (NAME) VALUES ('x')",
    ]);
}

#[test]
fn sqlite_valid_joins_subqueries_ctes_windows() {
    sqlite_clean(&[
        "SELECT a.name, p.title FROM authors a JOIN posts p ON p.author_id = a.id",
        "SELECT a.name, p.title FROM authors a LEFT OUTER JOIN posts p ON p.author_id = a.id",
        "SELECT p.title, t.name FROM posts p JOIN post_tags pt ON pt.post_id = p.id JOIN tags t ON t.id = pt.tag_id",
        "SELECT a.name, p.title FROM authors a, posts p WHERE p.author_id = a.id",
        "SELECT p.id FROM posts p CROSS JOIN tags t",
        "SELECT id, title FROM published_posts",
        "SELECT pp.title, a.name FROM published_posts pp JOIN authors a ON a.id = pp.author_id",
        "SELECT name FROM authors WHERE id IN (SELECT author_id FROM posts WHERE status = 'published')",
        "SELECT name FROM authors a WHERE NOT EXISTS (SELECT 1 FROM posts p WHERE p.author_id = a.id)",
        "SELECT name, (SELECT COUNT(*) FROM posts p WHERE p.author_id = a.id) AS n FROM authors a",
        "WITH counts AS (SELECT author_id, COUNT(*) AS n FROM posts GROUP BY author_id) \
         SELECT a.name, c.n FROM authors a JOIN counts c ON c.author_id = a.id",
        "WITH RECURSIVE cnt(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM cnt WHERE x < 5) SELECT x FROM cnt",
        "SELECT id, ROW_NUMBER() OVER (PARTITION BY author_id ORDER BY published_at DESC) FROM posts",
        "SELECT id, SUM(view_count) OVER (ORDER BY id ROWS UNBOUNDED PRECEDING) FROM posts",
        "SELECT name FROM tags UNION SELECT title FROM posts",
        "SELECT name FROM tags EXCEPT SELECT title FROM posts",
        "SELECT author_id, COUNT(*) FROM posts GROUP BY author_id HAVING COUNT(*) > 2",
        "SELECT DISTINCT author_id FROM posts ORDER BY author_id DESC",
    ]);
}

// =====================================================================
// SQLite invalid queries
// =====================================================================

#[test]
fn sqlite_invalid_table_and_column_not_found() {
    use DiagnosticKind::*;
    sqlite_bad(&[
        bad(
            "SELECT name FROM authors a\nCROSS JOIN author",
            TableNotFound,
            "author",
            None,
            Some((2, 12)),
        ),
        bad(
            "INSERT INTO tag (name) VALUES ('x')",
            TableNotFound,
            "tag",
            Some("Did you mean 'tags'?"),
            Some((1, 13)),
        ),
        bad("DELETE FROM post", TableNotFound, "post", None, None),
        bad(
            "SELECT p.id FROM posts p\n  CROSS JOIN post_tag",
            TableNotFound,
            "post_tag",
            None,
            Some((2, 14)),
        ),
        bad(
            "SELECT nme FROM authors",
            ColumnNotFound,
            "nme",
            Some("Did you mean 'name'?"),
            Some((1, 8)),
        ),
        bad(
            "SELECT id FROM posts WHERE auther_id = 1",
            ColumnNotFound,
            "auther_id",
            Some("Did you mean 'author_id'?"),
            Some((1, 28)),
        ),
        bad(
            "INSERT INTO posts (author_id, titel) VALUES (1, 'x')",
            ColumnNotFound,
            "titel",
            Some("Did you mean 'title'?"),
            Some((1, 31)),
        ),
        bad(
            "UPDATE posts SET view_cnt = 0",
            ColumnNotFound,
            "view_cnt",
            Some("Did you mean 'view_count'?"),
            Some((1, 18)),
        ),
        bad(
            "SELECT p.ratin FROM posts p",
            ColumnNotFound,
            "ratin",
            Some("Did you mean 'rating'?"),
            None,
        ),
        bad(
            "SELECT id FROM posts WHERE author_id = ?1\n  AND publshed_at > datetime('now')",
            ColumnNotFound,
            "publshed_at",
            Some("Did you mean 'published_at'?"),
            Some((2, 7)),
        ),
        bad(
            "SELECT title FROM published_posts WHERE status = 'x'",
            ColumnNotFound,
            "status",
            None,
            None,
        ),
        bad(
            "SELECT post_id FROM post_tags WHERE tagid = 1",
            ColumnNotFound,
            "tagid",
            Some("Did you mean 'tag_id'?"),
            None,
        ),
    ]);
}

#[test]
fn sqlite_invalid_type_count_ambiguity_null() {
    use DiagnosticKind::*;
    sqlite_bad(&[
        bad(
            "SELECT id FROM authors WHERE name = 5",
            TypeMismatch,
            "cannot compare",
            Some("CAST"),
            Some((1, 30)),
        ),
        bad(
            "SELECT id FROM posts WHERE view_count = 'many'",
            TypeMismatch,
            "cannot compare",
            None,
            Some((1, 28)),
        ),
        bad(
            "SELECT title * 2 FROM posts",
            TypeMismatch,
            "Arithmetic",
            None,
            Some((1, 8)),
        ),
        bad(
            "INSERT INTO posts (author_id, title, view_count) VALUES (1, 't', 'lots')",
            TypeMismatch,
            "view_count",
            None,
            None,
        ),
        bad(
            "UPDATE posts SET author_id = 'bob' WHERE id = 1",
            TypeMismatch,
            "author_id",
            None,
            None,
        ),
        bad(
            "SELECT p.id FROM posts p JOIN authors a ON a.name = p.author_id",
            JoinTypeMismatch,
            "text vs integer",
            Some("CAST"),
            Some((1, 44)),
        ),
        bad(
            "INSERT INTO tags (id, name) VALUES (1)",
            ColumnCountMismatch,
            "1 value(s) but 2 column(s)",
            None,
            None,
        ),
        bad(
            "INSERT INTO post_tags VALUES (1, 2, 3)",
            ColumnCountMismatch,
            "3 value(s) but 2 column(s)",
            Some("has 2 columns"),
            None,
        ),
        bad(
            "SELECT name FROM authors a JOIN tags t ON t.id = a.id",
            AmbiguousColumn,
            "name",
            Some("Qualify the column"),
            Some((1, 8)),
        ),
        bad(
            "INSERT INTO posts (author_id, title) VALUES (1, NULL)",
            PotentialNullViolation,
            "title",
            Some("NOT NULL"),
            None,
        ),
        bad(
            "UPDATE posts SET status = NULL WHERE id = ?",
            PotentialNullViolation,
            "status",
            None,
            None,
        ),
    ]);
}

// =====================================================================
// Schema resilience
// =====================================================================

#[test]
fn mysql_schema_resilient_to_dump_noise() {
    // Triggers with DELIMITER, mysqldump-style views and escaped string data
    // must not prevent the surrounding tables from being registered.
    let ddl = r"
CREATE TABLE `a` (`id` int NOT NULL, PRIMARY KEY (`id`)) ENGINE=InnoDB;
DELIMITER ;;
/*!50003 CREATE*/ /*!50017 DEFINER=`root`@`localhost`*/ /*!50003 TRIGGER `a_bi` BEFORE INSERT ON `a` FOR EACH ROW SET NEW.id = NEW.id + 1 */;;
DELIMITER ;
CREATE TABLE `b` (`id` int NOT NULL, `a_id` int DEFAULT NULL) ENGINE=InnoDB;
";
    let catalog = build_catalog(SqlDialect::MySQL, ddl);
    for name in ["a", "b"] {
        assert!(
            catalog.table_exists(&QualifiedName::new(name)),
            "table `{name}` should exist: {:?}",
            catalog.table_names()
        );
    }
}

#[test]
fn sqlite_schema_resilient_to_unsupported_statements() {
    let ddl = r"
PRAGMA foreign_keys=OFF;
CREATE TABLE a (id INTEGER PRIMARY KEY, n INT);
CREATE VIRTUAL TABLE docs USING fts5(title, body);
ATTACH DATABASE 'other.db' AS other;
CREATE TABLE b (id INTEGER PRIMARY KEY, a_id INTEGER REFERENCES a(id));
VACUUM;
";
    let catalog = build_catalog(SqlDialect::SQLite, ddl);
    for name in ["a", "b"] {
        assert!(
            catalog.table_exists(&QualifiedName::new(name)),
            "table `{name}` should exist: {:?}",
            catalog.table_names()
        );
    }
    let catalog_ref = &catalog;
    assert_all_clean(
        catalog_ref,
        SqlDialect::SQLite,
        &["SELECT b.id, a.n FROM b JOIN a ON a.id = b.a_id"],
    );
}

#[test]
fn mysql_valid_expressions_and_dml_misc() {
    mysql_clean(&[
        "SELECT id FROM products WHERE long_description LIKE '%x%'",
        "SELECT id FROM products WHERE is_active",
        "SELECT id FROM products WHERE NOT is_active",
        "SELECT id FROM orders WHERE placed_at + INTERVAL 1 DAY > shipped_at",
        "SELECT placed_at + INTERVAL 1 DAY FROM orders",
        "SELECT `o`.`id` FROM `orders` `o` WHERE `o`.`status` = 'paid'",
        "SELECT id FROM customers WHERE email LIKE CONCAT('%', ?, '%')",
        "SELECT o.id FROM orders o, customers c WHERE o.customer_id = c.id",
        "SELECT id, email FROM customers WHERE (status, is_verified) = ('active', 1)",
        "SELECT * FROM (SELECT id, total FROM orders) AS t ORDER BY t.total DESC LIMIT 5",
        "SELECT CASE status WHEN 'paid' THEN 1 ELSE 0 END AS paid FROM orders",
        "SELECT id FROM customers WHERE id NOT IN (SELECT customer_id FROM orders)",
        "SELECT JSON_LENGTH(profile) FROM customers",
        "SELECT id FROM customers WHERE JSON_EXTRACT(profile, '$.age') > 18",
        "SELECT CURRENT_DATE, CURRENT_TIME, CURRENT_TIMESTAMP",
        "SELECT MATCH(name) AGAINST ('widget') FROM products",
        "SELECT id FROM customers WHERE email <=> NULL",
        "SELECT BINARY email FROM customers",
        "SELECT id FROM customers WHERE email COLLATE utf8mb4_bin = 'A@x.io'",
        "SELECT CAST(created_at AS DATE) FROM customers",
        "SELECT CONVERT(price, DECIMAL(10, 2)) FROM products",
        "SELECT EXTRACT(YEAR FROM placed_at) FROM orders",
        "SELECT id FROM orders o WHERE o.total > (SELECT AVG(total) FROM orders)",
        "SELECT c.id, COUNT(o.id) FROM customers c LEFT JOIN orders o ON o.customer_id = c.id GROUP BY c.id HAVING COUNT(o.id) = 0",
        "SELECT id FROM customers ORDER BY id DESC LIMIT 1",
        "UPDATE customers c JOIN orders o ON o.customer_id = c.id SET c.balance = c.balance + o.total WHERE o.status = 'paid'",
        "DELETE c, o FROM customers c JOIN orders o ON o.customer_id = c.id WHERE c.id = 1",
        "SELECT LAST_INSERT_ID()",
        "SELECT VERSION(), DATABASE(), USER()",
        "SELECT id FROM customers WHERE created_at > NOW() - INTERVAL 30 DAY",
        "INSERT INTO audit_log (actor, action, payload) VALUES ('me', 'login', CAST('{}' AS JSON))",
        "INSERT INTO customers (email, full_name) VALUES ('a', 'b') ON DUPLICATE KEY UPDATE full_name = VALUES(full_name), balance = balance + 1",
        "INSERT INTO customers (email, full_name) VALUES ('a', 'b') AS new ON DUPLICATE KEY UPDATE full_name = new.full_name",
    ]);
}

#[test]
fn mysql_invalid_in_nested_and_dml_contexts() {
    use DiagnosticKind::*;
    mysql_bad(&[
        bad("INSERT INTO products (sku, name, price, stock) VALUES ('S', 'N', 1, UPPER('x'))", TypeMismatch, "stock", None, Some((1, 69))),
        bad("UPDATE order_items SET quantity = LOWER('x') WHERE order_id = 1", TypeMismatch, "quantity", None, Some((1, 35))),
        bad("UPDATE customers c JOIN orders o ON o.customer_id = c.id SET c.balanse = 1", ColumnNotFound, "balanse", Some("Did you mean 'balance'?"), None),
        bad("SELECT customer_id, GROUP_CONCAT(stats) FROM orders GROUP BY customer_id", ColumnNotFound, "stats", Some("Did you mean 'status'?"), None),
        bad("SELECT id, ROW_NUMBER() OVER (PARTITION BY custmer_id ORDER BY id) FROM orders", ColumnNotFound, "custmer_id", None, None),
        bad("SELECT id, ROW_NUMBER() OVER (PARTITION BY customer_id ORDER BY placd_at) FROM orders", ColumnNotFound, "placd_at", None, None),
        bad("SELECT t.n FROM (SELECT customer_id, COUNT(*) AS cnt FROM orders GROUP BY customer_id) t", ColumnNotFound, "n", None, None),
        bad("SELECT profle->>'$.plan' FROM customers", ColumnNotFound, "profle", Some("Did you mean 'profile'?"), Some((1, 8))),
        bad("SELECT JSON_EXTRACT(atributes, '$.c') FROM products", ColumnNotFound, "atributes", Some("Did you mean 'attributes'?"), None),
        bad("SELECT customer_id FROM orders GROUP BY customer_id HAVING SUM(totl) > 1", ColumnNotFound, "totl", None, None),
        bad("INSERT INTO audit_log (actor, action, payload) SELECT ful_name, 'x', '{}' FROM customers", ColumnNotFound, "ful_name", None, None),
        bad("SELECT id FROM orders WHERE status = 'paid' AND customer_id = 'abc'", TypeMismatch, "cannot compare", None, Some((1, 49))),
        bad("SELECT IFNULL(totl, 0) FROM orders", ColumnNotFound, "totl", None, None),
        bad("SELECT DATE_FORMAT(placed, '%Y') FROM orders", ColumnNotFound, "placed", None, None),
        bad("DELETE o FROM orders o JOIN customers c ON c.id = o.customer_id WHERE c.stat = 'x'", ColumnNotFound, "stat", None, None),
        bad("SELECT c.id FROM customers c JOIN orders o ON o.customer_id = c.id WHERE x.id = 1", TableNotFound, "x", None, None),
        bad("INSERT INTO customers (email, full_name, is_verified) VALUES ('a', 'b', 'yes')", TypeMismatch, "is_verified", None, None),
        bad("SELECT id FROM customers WHERE id = ? AND email = 1", TypeMismatch, "cannot compare", None, Some((1, 43))),
        bad("WITH t AS (SELECT id FROM orders) SELECT id FROM t JOIN customers c ON c.id = t.id", AmbiguousColumn, "id", None, None),
        bad("SELECT id FROM orders o\nWHERE o.customer_id IN (\n  SELECT c.id FROM customers c WHERE c.emial = ?\n)", ColumnNotFound, "emial", Some("Did you mean 'email'?"), Some((3, 40))),
    ]);
}

#[test]
fn sqlite_valid_expressions_and_dml_misc() {
    sqlite_clean(&[
        "SELECT id FROM posts WHERE published_at > datetime('now')",
        "SELECT id FROM authors WHERE created_at > '2024-01-01'",
        "SELECT id FROM authors WHERE is_active = FALSE",
        "INSERT INTO authors (name, is_active) VALUES ('x', TRUE)",
        "SELECT id FROM posts WHERE metadata ->> '$.lang' = 'en'",
        "SELECT p.id FROM posts p JOIN json_each(p.metadata) j",
        "SELECT id FROM posts WHERE title LIKE ? || '%'",
        "SELECT id FROM posts WHERE title REGEXP '^a'",
        "SELECT id FROM posts WHERE rating IS NULL ORDER BY rating NULLS LAST",
        "SELECT id FROM posts ORDER BY id LIMIT 10, 5",
        "SELECT count(*) FILTER (WHERE status = 'draft') FROM posts",
        "SELECT id, ntile(4) OVER (ORDER BY view_count) FROM posts",
        "SELECT a.name FROM authors a WHERE a.id = (SELECT max(author_id) FROM posts)",
        "SELECT name FROM authors INTERSECT SELECT title FROM posts",
        "SELECT * FROM authors NATURAL JOIN posts",
        "UPDATE posts SET (title, body) = ('t', 'b') WHERE id = 1",
        "UPDATE posts SET view_count = view_count + 1 FROM authors WHERE authors.id = posts.author_id",
        "DELETE FROM posts WHERE id IN (SELECT post_id FROM post_tags WHERE tag_id = 1) RETURNING *",
        "SELECT CAST(strftime('%s', 'now') AS INTEGER)",
        "SELECT id FROM events WHERE occurred_at > strftime('%s', 'now') - 3600",
        "SELECT id FROM events WHERE occurred_at > unixepoch() - 3600",
        "SELECT id FROM posts WHERE view_count > 0 AND rating >= 3.5",
        "SELECT id FROM authors WHERE email IS NOT NULL AND email <> ''",
        "INSERT INTO post_tags VALUES (1, 2)",
        "INSERT INTO post_tags (post_id, tag_id) VALUES (?, ?), (?, ?)",
        "SELECT `id` FROM `posts`",
        "SELECT [id] FROM [posts]",
        "SELECT \"id\" FROM \"posts\"",
    ]);
}

#[test]
fn sqlite_invalid_in_nested_and_dml_contexts() {
    use DiagnosticKind::*;
    sqlite_bad(&[
        bad("INSERT OR REPLACE INTO kvs (key) VALUES ('a')", TableNotFound, "kvs", None, Some((1, 24))),
        bad("SELECT json_extract(metdata, '$.a') FROM posts", ColumnNotFound, "metdata", Some("Did you mean 'metadata'?"), None),
        bad("SELECT strftime('%Y', publishd_at) FROM posts", ColumnNotFound, "publishd_at", None, None),
        bad("SELECT id, ROW_NUMBER() OVER (ORDER BY ratng) FROM posts", ColumnNotFound, "ratng", None, None),
        bad("WITH c AS (SELECT author_id FROM posts) SELECT c.post_id FROM c", ColumnNotFound, "post_id", None, None),
        bad("SELECT title FROM published_posts WHERE body = 'x'", ColumnNotFound, "body", None, None),
        bad("SELECT id FROM posts WHERE author_id = :author AND titl = :t", ColumnNotFound, "titl", None, Some((1, 52))),
        bad("SELECT id FROM posts WHERE title = 1", TypeMismatch, "cannot compare", None, Some((1, 28))),
        bad("UPDATE posts SET view_count = upper(title) WHERE id = 1", TypeMismatch, "view_count", None, Some((1, 31))),
        bad("INSERT INTO post_tags (post_id, tag_id) VALUES (NULL, 1)", PotentialNullViolation, "post_id", None, None),
        bad("SELECT p.id FROM posts p JOIN tags t ON t.name = p.id", JoinTypeMismatch, "text vs integer", None, Some((1, 41))),
        bad("SELECT id FROM posts p JOIN post_tags pt ON pt.post_id = p.id JOIN tags t ON t.id = pt.tag_id", AmbiguousColumn, "id", None, Some((1, 8))),
    ]);
}

#[test]
fn mysql_valid_literals_comments_and_modifiers() {
    mysql_clean(&[
        "SELECT id DIV 2 FROM customers",
        "SELECT id XOR 1 FROM customers",
        "SELECT id FROM customers WHERE email = 'x' FOR SHARE",
        "SELECT HEX(ip), INET6_NTOA(ip) FROM audit_log",
        "SELECT id FROM customers WHERE email = _utf8mb4'x'",
        "SELECT id FROM customers WHERE email = 'O\\'Brien'",
        "SELECT id FROM customers WHERE email = 'it''s'",
        "SELECT id FROM customers WHERE created_at > TIMESTAMP '2024-01-01 00:00:00'",
        "SELECT id FROM customers WHERE birth_date > DATE '2000-01-01'",
        "SELECT id FROM orders WHERE total > 0x10",
        "SELECT id FROM orders WHERE total > 1e3",
        "SELECT id FROM products WHERE price > -1.5",
        "SELECT id FROM customers WHERE status = 'active' /* comment */ AND is_verified = 1 -- trailing\n",
        "SELECT id FROM customers WHERE status = 'active' # hash comment\n",
        "SELECT /*+ MAX_EXECUTION_TIME(1000) */ id FROM customers",
        "SELECT id FROM customers PARTITION (p0)",
        "SELECT name FROM products WHERE MATCH(name, description) AGAINST ('x' IN BOOLEAN MODE)",
        "SELECT COUNT(*) FROM customers WHERE is_verified",
    ]);
}

#[test]
fn mysql_valid_selects_more() {
    mysql_clean(&[
        "SELECT c.full_name, o.total FROM customers AS c LEFT OUTER JOIN orders AS o ON (o.customer_id = c.id AND o.status = 'paid')",
        "SELECT COUNT(*) AS n FROM orders WHERE status <> 'cancelled' AND placed_at IS NOT NULL",
        "INSERT INTO order_items (order_id, product_id, quantity, unit_price) SELECT 1, id, 1, price FROM products WHERE sku = ?",
        "INSERT INTO settings VALUES ('a', 'b', NULL)",
        "SELECT ANY_VALUE(full_name), status FROM customers GROUP BY status",
        "SELECT SUM(CASE WHEN status = 'paid' THEN total ELSE 0 END) FROM orders",
        "SELECT id FROM orders WHERE status IN (SELECT status FROM orders WHERE total > 100)",
        "SELECT `order_items`.`quantity` FROM `order_items`",
        "SELECT o.id FROM orders o JOIN customers c ON c.id = o.customer_id ORDER BY o.placed_at DESC, c.full_name",
        "SELECT status, COUNT(*) FROM orders GROUP BY 1",
        "SELECT * FROM customers WHERE id = 1",
        "SELECT c.* FROM customers c",
        "SELECT o.*, c.email FROM orders o JOIN customers c ON c.id = o.customer_id",
        "SELECT JSON_OBJECTAGG(`key`, `value`) FROM settings",
        "SELECT id FROM customers WHERE JSON_CONTAINS_PATH(profile, 'one', '$.a')",
        "SELECT id, total, total - LAG(total, 1, 0) OVER (ORDER BY id) AS delta FROM orders",
        "SELECT id, FIRST_VALUE(total) OVER (PARTITION BY customer_id ORDER BY placed_at \
         RANGE BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) FROM orders",
        "SELECT id FROM orders WHERE placed_at > shipped_at",
        "SELECT o.id FROM orders o JOIN orders o2 ON o2.placed_at = o.shipped_at",
        "SELECT LOWER(email) FROM customers WHERE LOWER(email) = LOWER(?)",
        "SELECT id FROM customers WHERE LENGTH(full_name) > 3",
        "SELECT id FROM customers WHERE CHAR_LENGTH(full_name) BETWEEN 1 AND 100",
        "SELECT id FROM products WHERE ROUND(price) = 10",
        "SELECT id FROM orders WHERE COALESCE(coupon_code, '') = ''",
        "SELECT id FROM customers WHERE id = 1; SELECT id FROM orders WHERE id = 2",
    ]);
}

#[test]
fn mysql_invalid_more_contexts() {
    use DiagnosticKind::*;
    mysql_bad(&[
        bad(
            "SELECT o.id FROM orders o JOIN customers c ON c.id = o.customer_id ORDER BY status",
            AmbiguousColumn,
            "status",
            Some("Qualify the column"),
            Some((1, 77)),
        ),
        bad(
            "SELECT c.id FROM customers c JOIN orders o ON o.customer_id = c.id GROUP BY status",
            AmbiguousColumn,
            "status",
            None,
            Some((1, 77)),
        ),
        bad(
            "INSERT INTO settings (`key`, `value`) VALUES ('a', 'b'), ('c')",
            ColumnCountMismatch,
            "1 value(s) but 2 column(s)",
            None,
            None,
        ),
        bad(
            "INSERT INTO products (sku, name, price) VALUES ('a', 'b')",
            ColumnCountMismatch,
            "2 value(s) but 3 column(s)",
            Some("Provide 3 value(s)"),
            None,
        ),
        bad(
            "INSERT INTO products VALUES (1)",
            ColumnCountMismatch,
            "1 value(s) but 14 column(s)",
            Some("Table 'products' has 14 columns"),
            None,
        ),
        bad(
            "INSERT INTO order_items (order_id, product_id, quantity, unit_price) VALUES (1, 2, 3, NULL)",
            PotentialNullViolation,
            "unit_price",
            None,
            None,
        ),
        bad(
            "UPDATE products SET name = NULL WHERE id = 1",
            PotentialNullViolation,
            "name",
            None,
            None,
        ),
        bad(
            "DELETE FROM customers WHERE balance = 'lots'",
            TypeMismatch,
            "numeric(12,2) with text",
            None,
            Some((1, 29)),
        ),
        bad(
            "SELECT id FROM customers WHERE created_at > 5",
            TypeMismatch,
            "timestamp with integer",
            None,
            Some((1, 32)),
        ),
        bad(
            "SELECT id FROM orders o JOIN order_items oi ON oi.order_id = o.id WHERE quantity > 'x'",
            TypeMismatch,
            "smallint with text",
            None,
            Some((1, 73)),
        ),
        bad(
            "SELECT p.id FROM products p JOIN order_items oi ON p.sku = oi.product_id",
            JoinTypeMismatch,
            "char(12) vs bigint",
            None,
            Some((1, 52)),
        ),
        bad(
            "SELECT t.id FROM (SELECT id FROM orders WHERE totl > 0) t",
            ColumnNotFound,
            "totl",
            Some("Did you mean 'total'?"),
            Some((1, 47)),
        ),
        bad(
            "SELECT `c`.`emial` FROM `customers` `c`",
            ColumnNotFound,
            "emial",
            Some("Did you mean 'email'?"),
            Some((1, 12)),
        ),
        bad(
            "SELECT id FROM orders HAVING totl > 1",
            ColumnNotFound,
            "totl",
            Some("Did you mean 'total'?"),
            Some((1, 30)),
        ),
        bad(
            "SELECT id FROM customers c WHERE NOT EXISTS (SELECT 1 FROM orders o WHERE o.custmer_id = c.id)",
            ColumnNotFound,
            "custmer_id",
            Some("Did you mean 'customer_id'?"),
            Some((1, 77)),
        ),
        bad(
            "SELECT (SELECT MAX(totl) FROM orders) FROM customers",
            ColumnNotFound,
            "totl",
            None,
            Some((1, 20)),
        ),
        bad(
            "SELECT c.id FROM customers c WHERE c.id = o.customer_id",
            TableNotFound,
            "'o'",
            None,
            Some((1, 43)),
        ),
        bad(
            "SELECT id FROM customers ORDER BY full_nam",
            ColumnNotFound,
            "full_nam",
            Some("Did you mean 'full_name'?"),
            Some((1, 35)),
        ),
        // MySQL does not allow SELECT aliases in WHERE.
        bad(
            "SELECT id, email AS e FROM customers WHERE e = 'x'",
            ColumnNotFound,
            "'e'",
            None,
            Some((1, 44)),
        ),
        bad(
            "SELECT id FROM customers WHERE id = 1; SELECT emial FROM customers",
            ColumnNotFound,
            "emial",
            None,
            Some((1, 47)),
        ),
        bad(
            "SELECT\n  c.id,\n  c.emial\nFROM customers c",
            ColumnNotFound,
            "emial",
            None,
            Some((3, 5)),
        ),
        bad(
            "SELECT id FROM customers WHERE id IN (SELECT customer_id FROM orders WHERE x.id = 1)",
            TableNotFound,
            "'x'",
            None,
            None,
        ),
    ]);
}

#[test]
fn mysql_inline_disable_directive() {
    mysql_clean(&[
        "SELECT `emial` FROM customers -- sqlsift:disable E0002",
        "-- sqlsift:disable E0001\nSELECT COUNT(*) FROM missing_table",
    ]);
}

#[test]
fn sqlite_valid_selects_more() {
    sqlite_clean(&[
        "SELECT * FROM posts WHERE id = 1",
        "SELECT p.* FROM posts p",
        "SELECT COUNT(*) FROM post_tags",
        "INSERT INTO events (payload) VALUES (zeroblob(10))",
        "SELECT id FROM events WHERE payload IS NULL",
        "SELECT a.name, COUNT(p.id) AS n FROM authors a LEFT JOIN posts p ON p.author_id = a.id GROUP BY a.id ORDER BY n DESC",
        "SELECT id FROM posts WHERE status = 'published' AND (rating > 3 OR view_count > 1000)",
        "SELECT title FROM posts WHERE id = (SELECT post_id FROM post_tags LIMIT 1)",
        "SELECT id FROM posts WHERE title NOT LIKE '%draft%'",
        "SELECT id FROM posts ORDER BY rating DESC NULLS FIRST",
        "SELECT CASE WHEN body IS NULL THEN 0 ELSE length(body) END FROM posts",
        "SELECT * FROM published_posts",
        "SELECT sqlite_version()",
        "SELECT id FROM posts WHERE author_id IN (?1, ?2)",
        "SELECT id FROM posts WHERE id = CAST(?1 AS INTEGER)",
        "DELETE FROM kv WHERE expires_at < unixepoch()",
        "UPDATE kv SET expires_at = NULL WHERE key = ?",
        "SELECT id FROM posts WHERE rating > 4 LIMIT -1 OFFSET 5",
        "INSERT INTO post_tags SELECT id, 1 FROM posts",
        "SELECT a.id FROM authors a WHERE a.id = 1 UNION ALL SELECT p.author_id FROM posts p",
        "SELECT id, rank() OVER win FROM posts WINDOW win AS (ORDER BY view_count DESC)",
        "SELECT x.id FROM (SELECT id FROM posts) AS x",
        "SELECT * FROM posts p LEFT JOIN authors a ON a.id = p.author_id WHERE a.id IS NULL",
        "SELECT id FROM posts WHERE EXISTS (SELECT 1 FROM post_tags WHERE post_id = posts.id)",
        "SELECT id FROM authors WHERE is_active IS TRUE",
        "SELECT id FROM posts WHERE author_id IS DISTINCT FROM 1",
        "SELECT id FROM authors WHERE created_at > datetime('now', '-1 day')",
        "SELECT id FROM posts WHERE published_at IS NOT NULL ORDER BY published_at DESC LIMIT 20",
        "SELECT title FROM posts WHERE title = 'x' COLLATE NOCASE",
        "SELECT id FROM posts WHERE id = 0x1F",
        "SELECT char(65), unicode('A'), soundex('x')",
        "SELECT p.id, j.value FROM posts p, json_each(p.metadata) AS j",
        "SELECT j.key, j.value FROM posts p, json_each(p.metadata) AS j WHERE j.key = 'lang'",
        "SELECT json_extract(metadata, '$.a') AS a FROM posts ORDER BY a",
        "SELECT key, value FROM kv",
        "INSERT INTO kv (key, expires_at) VALUES ('a', 1) ON CONFLICT (key) DO UPDATE SET expires_at = excluded.expires_at \
         WHERE excluded.expires_at > 0 RETURNING key",
    ]);
}

#[test]
fn sqlite_invalid_more_contexts() {
    use DiagnosticKind::*;
    sqlite_bad(&[
        bad(
            "SELECT a.id FROM authors a JOIN posts p ON p.author_id = a.id WHERE id = 1",
            AmbiguousColumn,
            "'id'",
            Some("Qualify the column"),
            Some((1, 69)),
        ),
        bad(
            "UPDATE authors SET name = NULL",
            PotentialNullViolation,
            "name",
            None,
            None,
        ),
        bad(
            "INSERT INTO kv (key, expires_at) VALUES ('a', 1, 2)",
            ColumnCountMismatch,
            "3 value(s) but 2 column(s)",
            None,
            None,
        ),
        bad(
            "SELECT id FROM posts ORDER BY ratng",
            ColumnNotFound,
            "ratng",
            Some("Did you mean 'rating'?"),
            Some((1, 31)),
        ),
        bad(
            "SELECT id FROM posts WHERE author_id IN (SELECT id FROM writer)",
            TableNotFound,
            "writer",
            None,
            Some((1, 57)),
        ),
        bad(
            "DELETE FROM kv WHERE expire_at < 0",
            ColumnNotFound,
            "expire_at",
            Some("Did you mean 'expires_at'?"),
            Some((1, 22)),
        ),
        // Double-quoted strings are identifiers in SQLite; the string-literal
        // fallback is a documented misfeature that is disabled with SQLITE_DQS=0.
        bad(
            "SELECT id FROM posts WHERE status = \"published\"",
            ColumnNotFound,
            "published",
            None,
            Some((1, 37)),
        ),
        bad(
            "SELECT id FROM posts WHERE view_count > 'ten'",
            TypeMismatch,
            "integer with text",
            None,
            Some((1, 28)),
        ),
        bad(
            "UPDATE posts SET rating = 'great'",
            TypeMismatch,
            "rating",
            None,
            None,
        ),
        bad(
            "INSERT INTO authors (name, is_active) VALUES ('x', 'maybe')",
            TypeMismatch,
            "is_active",
            None,
            None,
        ),
        bad(
            "SELECT a.id FROM authors a JOIN posts p ON a.created_at = p.id",
            JoinTypeMismatch,
            "text vs integer",
            None,
            Some((1, 44)),
        ),
        bad(
            "SELECT id FROM posts GROUP BY author HAVING count(*) > 1",
            ColumnNotFound,
            "author",
            Some("Did you mean 'author_id'?"),
            Some((1, 31)),
        ),
        bad(
            "SELECT id FROM posts WHERE id = ?1 AND auhtor_id = ?2",
            ColumnNotFound,
            "auhtor_id",
            Some("Did you mean 'author_id'?"),
            Some((1, 40)),
        ),
    ]);
}

// =====================================================================
// DDL feature coverage (per-snippet schemas)
// =====================================================================

/// (dialect, table to look up, DDL, expected column names in order)
type DdlCase = (
    SqlDialect,
    &'static str,
    &'static str,
    &'static [&'static str],
);

#[test]
fn ddl_features_register_tables_and_columns() {
    let cases: &[DdlCase] = &[
        (
            SqlDialect::MySQL,
            "t",
            "CREATE TABLE `t` (`a` int, `b` int AS (`a` * 2) VIRTUAL, \
             `c` varchar(10) GENERATED ALWAYS AS (concat(`a`, 'x')) VIRTUAL)",
            &["a", "b", "c"],
        ),
        (
            SqlDialect::MySQL,
            "t",
            "CREATE TABLE t (a int, b text, FULLTEXT KEY `ft_b` (`b`)) ENGINE=InnoDB",
            &["a", "b"],
        ),
        (
            SqlDialect::MySQL,
            "t",
            "CREATE TABLE t (a int, SPATIAL KEY sp (a))",
            &["a"],
        ),
        (
            SqlDialect::MySQL,
            "t",
            "CREATE TABLE t (a int NOT NULL, CONSTRAINT `t_chk_1` CHECK ((`a` > 0)))",
            &["a"],
        ),
        (
            SqlDialect::MySQL,
            "t",
            "CREATE TABLE t (a varchar(10) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL DEFAULT '')",
            &["a"],
        ),
        (
            SqlDialect::MySQL,
            "t",
            "CREATE TABLE t (a int, b int, UNIQUE INDEX uq (a, b), INDEX idx_b USING BTREE (b))",
            &["a", "b"],
        ),
        (
            SqlDialect::MySQL,
            "t",
            "CREATE TABLE t (a int, b datetime DEFAULT NULL ON UPDATE CURRENT_TIMESTAMP)",
            &["a", "b"],
        ),
        (
            SqlDialect::MySQL,
            "t",
            "CREATE TABLE t (a BIT(1), b year, c time(3), d mediumblob, e longblob, f binary(16), h numeric(5))",
            &["a", "b", "c", "d", "e", "f", "h"],
        ),
        (
            SqlDialect::MySQL,
            "t",
            "CREATE TABLE t (a timestamp NULL DEFAULT NULL, b varchar(36) DEFAULT (uuid()), c bool)",
            &["a", "b", "c"],
        ),
        (
            SqlDialect::MySQL,
            "t",
            "CREATE TABLE t (a int);\nALTER TABLE `t` ADD COLUMN `b` varchar(10) NOT NULL AFTER `a`",
            &["a", "b"],
        ),
        (
            SqlDialect::MySQL,
            "t",
            "CREATE TABLE t (a int);\nALTER TABLE t ADD KEY idx_a (a), ADD CONSTRAINT fk FOREIGN KEY (a) REFERENCES u (id)",
            &["a"],
        ),
        (
            SqlDialect::MySQL,
            "t",
            "CREATE TEMPORARY TABLE t (a int)",
            &["a"],
        ),
        (
            SqlDialect::MySQL,
            "b",
            "CREATE TABLE a (id int);\nCREATE PROCEDURE p() BEGIN SELECT 1; END;\nCREATE TABLE b (id int);",
            &["id"],
        ),
        (
            SqlDialect::MySQL,
            "b",
            "CREATE TABLE a (id int);\nDELIMITER ;;\nCREATE TRIGGER tr BEFORE INSERT ON a FOR EACH ROW \
             BEGIN SET NEW.id = 1; END ;;\nDELIMITER ;\nCREATE TABLE b (id int);",
            &["id"],
        ),
        (
            SqlDialect::MySQL,
            "c",
            "CREATE TABLE a (id int) COMMENT 'it''s; fine';\n\
             CREATE TABLE b (id int) ROW_FORMAT=DYNAMIC;\nCREATE TABLE c (id int);",
            &["id"],
        ),
        (
            SqlDialect::SQLite,
            "t",
            "CREATE TABLE t (a INTEGER PRIMARY KEY ASC, b TEXT DEFAULT 'x' NOT NULL)",
            &["a", "b"],
        ),
        (
            SqlDialect::SQLite,
            "t",
            "CREATE TABLE t (a INTEGER PRIMARY KEY ON CONFLICT REPLACE, b INTEGER NOT NULL ON CONFLICT IGNORE)",
            &["a", "b"],
        ),
        (
            SqlDialect::SQLite,
            "t",
            "CREATE TABLE t (a TEXT, b TEXT GENERATED ALWAYS AS (upper(a)) VIRTUAL, c TEXT AS (lower(a)) STORED)",
            &["a", "b", "c"],
        ),
        (
            SqlDialect::SQLite,
            "t",
            "CREATE TABLE \"t\" (\"a\" INTEGER, [b] TEXT, `c` REAL)",
            &["a", "b", "c"],
        ),
        (
            SqlDialect::SQLite,
            "t",
            "CREATE TABLE t (a NVARCHAR(10), b CLOB, c INT8, d NUMERIC, e DECIMAL(10,5), f DOUBLE, g FLOAT)",
            &["a", "b", "c", "d", "e", "f", "g"],
        ),
        (
            SqlDialect::SQLite,
            "t",
            "CREATE TABLE t (a INTEGER, FOREIGN KEY (a) REFERENCES u(id) ON DELETE SET NULL DEFERRABLE INITIALLY DEFERRED)",
            &["a"],
        ),
        (
            SqlDialect::SQLite,
            "t",
            "CREATE TEMP TABLE t (a)",
            &["a"],
        ),
        (
            SqlDialect::SQLite,
            "t",
            "CREATE TABLE t (a TEXT UNIQUE NOT NULL, b TEXT CHECK (b <> '') NOT NULL)",
            &["a", "b"],
        ),
        (
            SqlDialect::SQLite,
            "t",
            "CREATE TABLE t (a INT);\nALTER TABLE t ADD COLUMN b TEXT DEFAULT 'x'",
            &["a", "b"],
        ),
        (
            SqlDialect::SQLite,
            "t2",
            "CREATE TABLE t (a INT);\nALTER TABLE t RENAME TO t2",
            &["a"],
        ),
        (
            SqlDialect::SQLite,
            "t",
            "CREATE TABLE t (a INT, b INT);\nALTER TABLE t RENAME COLUMN b TO c",
            &["a", "c"],
        ),
        (
            SqlDialect::SQLite,
            "t",
            "CREATE TABLE t (a INT, b INT);\nALTER TABLE t DROP COLUMN b",
            &["a"],
        ),
        (
            SqlDialect::SQLite,
            "b",
            "CREATE TABLE a (id INT);\nCREATE TRIGGER tr AFTER INSERT ON a BEGIN INSERT INTO a VALUES (1); \
             UPDATE a SET id = 2; END;\nCREATE TABLE b (id INT);",
            &["id"],
        ),
        (
            SqlDialect::SQLite,
            "a",
            "CREATE TABLE a (id INT, ts TEXT DEFAULT (datetime('now','localtime')), n INT DEFAULT -1, r REAL DEFAULT +1.5)",
            &["id", "ts", "n", "r"],
        ),
    ];
    let mut failures = Vec::new();
    for (dialect, name, ddl, expected) in cases {
        let catalog = build_catalog(*dialect, ddl);
        match catalog.get_table(&QualifiedName::new(*name)) {
            Some(t) => {
                let cols = t.column_names();
                if cols != *expected {
                    failures.push(format!(
                        "[{dialect}] {ddl}\n  expected columns {expected:?}, got {cols:?}"
                    ));
                }
            }
            None => failures.push(format!(
                "[{dialect}] {ddl}\n  table `{name}` missing; tables = {:?}",
                catalog.table_names()
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn ddl_views_are_registered() {
    let cases: &[(SqlDialect, &str)] = &[
        (
            SqlDialect::MySQL,
            "CREATE TABLE t (a int);\nCREATE OR REPLACE VIEW v AS SELECT a FROM t",
        ),
        (
            SqlDialect::MySQL,
            "CREATE TABLE t (a int);\nCREATE VIEW `v` AS select `t`.`a` AS `a` from `t`",
        ),
        (
            SqlDialect::SQLite,
            "CREATE TABLE t (a INT);\nCREATE VIEW IF NOT EXISTS v AS SELECT a FROM t;",
        ),
        (
            SqlDialect::SQLite,
            "CREATE TABLE t (a INT);\nCREATE TEMP VIEW v AS SELECT a FROM t;",
        ),
    ];
    for (dialect, ddl) in cases {
        let catalog = build_catalog(*dialect, ddl);
        assert!(
            catalog.view_exists(&QualifiedName::new("v")),
            "[{dialect}] view `v` should exist for: {ddl}"
        );
        let mut analyzer = Analyzer::with_dialect(&catalog, *dialect);
        let ok = analyzer.analyze("SELECT a FROM v");
        assert!(ok.is_empty(), "[{dialect}] {ddl}\n{}", fmt_diags(&ok));
        let bad = analyzer.analyze("SELECT b FROM v");
        assert!(
            bad.len() == 1 && bad[0].kind == DiagnosticKind::ColumnNotFound,
            "[{dialect}] {ddl}\n{}",
            fmt_diags(&bad)
        );
    }
}
