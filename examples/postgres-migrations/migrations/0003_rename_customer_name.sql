-- Customers now have a display name besides their legal name.
ALTER TABLE customers RENAME COLUMN name TO full_name;
ALTER TABLE customers ADD COLUMN display_name VARCHAR(100);
