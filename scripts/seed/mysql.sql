-- Fixture schema for Faro's MySQL and MariaDB tests.
--
-- Mirrors the Postgres fixture, with MySQL's own spellings and the cases that
-- catch drivers out: unsigned integers past i64, a table with no primary key,
-- a composite key, an enum, JSON, and a blob.

DROP TABLE IF EXISTS book_stores;
DROP TABLE IF EXISTS books;
DROP TABLE IF EXISTS authors;
DROP TABLE IF EXISTS access_log;
DROP TABLE IF EXISTS type_gallery;
DROP VIEW IF EXISTS books_with_authors;

CREATE TABLE authors (
    id          INT AUTO_INCREMENT PRIMARY KEY,
    name        VARCHAR(200) NOT NULL,
    email       VARCHAR(200) UNIQUE,
    bio         TEXT,
    created_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE books (
    id          INT AUTO_INCREMENT PRIMARY KEY,
    author_id   INT NOT NULL,
    title       VARCHAR(200) NOT NULL,
    isbn        CHAR(13),
    price       DECIMAL(10, 2),
    published   DATE,
    in_stock    BOOLEAN NOT NULL DEFAULT TRUE,
    metadata    JSON,
    cover       BLOB,
    CONSTRAINT books_author_fk FOREIGN KEY (author_id)
        REFERENCES authors(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE INDEX books_title_idx ON books(title);

-- Composite primary key.
CREATE TABLE book_stores (
    book_id     INT NOT NULL,
    store_id    INT NOT NULL,
    quantity    INT NOT NULL DEFAULT 0,
    PRIMARY KEY (book_id, store_id),
    CONSTRAINT book_stores_book_fk FOREIGN KEY (book_id) REFERENCES books(id)
) ENGINE=InnoDB;

-- No primary key: the editable grid must fall back to read-only.
CREATE TABLE access_log (
    ts          DATETIME,
    path        VARCHAR(255),
    status      SMALLINT,
    duration_ms DOUBLE
) ENGINE=InnoDB;

CREATE TABLE type_gallery (
    id             INT AUTO_INCREMENT PRIMARY KEY,
    a_tinyint      TINYINT,
    a_smallint     SMALLINT,
    a_int          INT,
    a_bigint       BIGINT,
    -- Past i64::MAX on purpose: decoding must not wrap it negative.
    a_ubigint      BIGINT UNSIGNED,
    a_float        FLOAT,
    a_double       DOUBLE,
    a_decimal      DECIMAL(30, 10),
    a_bool         BOOLEAN,
    a_date         DATE,
    a_time         TIME,
    a_datetime     DATETIME,
    a_timestamp    TIMESTAMP NULL,
    a_char         CHAR(10),
    a_varchar      VARCHAR(100),
    a_text         TEXT,
    a_enum         ENUM('small', 'medium', 'large'),
    a_json         JSON,
    a_blob         BLOB
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE VIEW books_with_authors AS
SELECT b.id, b.title, b.price, a.name AS author
FROM books b JOIN authors a ON a.id = b.author_id;

INSERT INTO authors (name, email, bio) VALUES
    ('Ada Lovelace',      'ada@example.com',    'Wrote the first algorithm.'),
    ('Grace Hopper',      'grace@example.com',  'Coined the term "debugging".'),
    ('Ursula K. Le Guin', 'ursula@example.com', NULL),
    ('Ken O''Brien',      'ken@example.com',    'Name contains an apostrophe.'),
    ('Ólafur Þórðarson',  'olafur@example.com', 'Unicode: þæö ÞÆÖ 日本語 🎉');

INSERT INTO books (author_id, title, isbn, price, published, metadata) VALUES
    (1, 'Notes on the Analytical Engine', '9780000000001', 42.50, '1843-10-01', '{"rare": true}'),
    (2, 'Compiling for Humans',           '9780000000002', 31.00, '1952-06-15', '{"edition": 2}'),
    (3, 'The Dispossessed',               '9780000000003', 18.99, '1974-01-01', NULL),
    (3, 'A Wizard of Earthsea',           '9780000000004', 15.25, '1968-01-01', NULL),
    (4, 'Quotes ''n'' Things',            NULL,            NULL,  NULL,         NULL);

INSERT INTO book_stores (book_id, store_id, quantity) VALUES
    (1, 1, 3), (1, 2, 0), (2, 1, 7), (3, 1, 12), (3, 2, 5);

INSERT INTO type_gallery (
    a_tinyint, a_smallint, a_int, a_bigint, a_ubigint, a_float, a_double,
    a_decimal, a_bool, a_date, a_time, a_datetime, a_timestamp,
    a_char, a_varchar, a_text, a_enum, a_json, a_blob
) VALUES (
    127, 32767, 2147483647, 9223372036854775807,
    -- Beyond i64::MAX; must survive as an exact value, not wrap negative.
    18446744073709551615,
    3.14, 2.718281828459045,
    12345678901234567890.0987654321,
    TRUE, '2026-08-05', '13:45:00', '2026-08-05 13:45:00', '2026-08-05 13:45:00',
    'fixed', 'variable', 'long text 🎉', 'medium', '{"k": "v"}', X'DEADBEEF'
), (
    NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
    NULL, NULL, NULL, NULL, NULL, NULL, NULL
);

-- Enough rows to page through and to exercise the truncation indicator.
--
-- Built by cross-joining digits rather than with a recursive CTE: MySQL caps
-- recursion at 1000 iterations by default, and MariaDB does not even recognise
-- the variable that raises it. Derived tables work identically on both.
INSERT INTO access_log (ts, path, status, duration_ms)
SELECT NOW(), CONCAT('/page/', n), 200 + (n % 5), RAND() * 500
FROM (
    SELECT d1.d + d2.d * 10 + d3.d * 100 + d4.d * 1000 + 1 AS n
    FROM (SELECT 0 AS d UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3
          UNION ALL SELECT 4 UNION ALL SELECT 5 UNION ALL SELECT 6
          UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9) d1
    CROSS JOIN (SELECT 0 AS d UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3
          UNION ALL SELECT 4 UNION ALL SELECT 5 UNION ALL SELECT 6
          UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9) d2
    CROSS JOIN (SELECT 0 AS d UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3
          UNION ALL SELECT 4 UNION ALL SELECT 5 UNION ALL SELECT 6
          UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9) d3
    CROSS JOIN (SELECT 0 AS d UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3
          UNION ALL SELECT 4 UNION ALL SELECT 5) d4
) nums
WHERE n <= 5000;
