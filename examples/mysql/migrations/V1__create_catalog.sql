CREATE TABLE categories (
    id         INT UNSIGNED NOT NULL AUTO_INCREMENT,
    slug       VARCHAR(64)  NOT NULL,
    name       VARCHAR(128) NOT NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_categories_slug (slug)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE products (
    id          INT UNSIGNED   NOT NULL AUTO_INCREMENT,
    category_id INT UNSIGNED   NOT NULL,
    sku         VARCHAR(32)    NOT NULL,
    title       VARCHAR(255)   NOT NULL,
    price       DECIMAL(10, 2) NOT NULL,
    stock       INT            NOT NULL DEFAULT 0,
    status      ENUM('active', 'hidden', 'discontinued') NOT NULL DEFAULT 'active',
    created_at  DATETIME       NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  DATETIME       NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    PRIMARY KEY (id),
    UNIQUE KEY uq_products_sku (sku),
    KEY idx_products_category (category_id),
    CONSTRAINT fk_products_category FOREIGN KEY (category_id) REFERENCES categories (id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
