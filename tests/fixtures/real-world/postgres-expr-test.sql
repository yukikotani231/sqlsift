-- =============================================================================
-- PostgreSQL Expression Coverage Test
-- Tests newly added Expr variants for column reference resolution
-- =============================================================================

-- Schema
CREATE TABLE events (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    data JSONB,
    tags TEXT[],
    score DECIMAL(10, 2),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE locations (
    id SERIAL PRIMARY KEY,
    city TEXT NOT NULL,
    country TEXT NOT NULL,
    coordinates POINT
);

-- =============================================================================
-- Pattern 61: AT TIME ZONE with column reference
-- =============================================================================
SELECT
    id,
    created_at AT TIME ZONE 'UTC' AS utc_time,
    created_at AT TIME ZONE 'America/New_York' AS eastern_time
FROM events;

-- =============================================================================
-- Pattern 62: COLLATE with column reference
-- =============================================================================
SELECT id, name
FROM events
WHERE name COLLATE "C" > 'A';

-- =============================================================================
-- Pattern 63: CEIL / FLOOR with column reference
-- =============================================================================
SELECT
    id,
    CEIL(score) AS ceil_score,
    FLOOR(score) AS floor_score
FROM events;

-- =============================================================================
-- Pattern 64: OVERLAY with column references
-- =============================================================================
SELECT
    id,
    OVERLAY(name PLACING '***' FROM 2 FOR 3) AS masked_name
FROM events;

-- =============================================================================
-- Pattern 65: IS DISTINCT FROM / IS NOT DISTINCT FROM
-- =============================================================================
SELECT e1.id
FROM events e1
JOIN events e2 ON e1.id = e2.id
WHERE e1.name IS DISTINCT FROM e2.name;

-- =============================================================================
-- Pattern 66: IS UNKNOWN
-- =============================================================================
SELECT id, name
FROM events
WHERE (score > 100) IS NOT UNKNOWN;

-- =============================================================================
-- Pattern 67: SIMILAR TO
-- =============================================================================
SELECT id, name
FROM events
WHERE name SIMILAR TO '%test%';

-- =============================================================================
-- Pattern 68: Tuple comparison (ROW)
-- =============================================================================
SELECT id, name, created_at
FROM events
WHERE (name, id) > ('test', 0);

-- =============================================================================
-- Pattern 69: ARRAY expression with column references
-- =============================================================================
SELECT id, name
FROM events
WHERE id = ANY(ARRAY[1, 2, 3]);

-- =============================================================================
-- Pattern 70: Array subscript
-- =============================================================================
SELECT
    id,
    tags[1] AS first_tag
FROM events;

-- =============================================================================
-- Pattern 71: GROUPING SETS expressions in GROUP BY
-- (column references in CUBE/ROLLUP expressions)
-- =============================================================================
SELECT
    name,
    score,
    COUNT(*) AS cnt
FROM events
GROUP BY CUBE (name, score);

-- =============================================================================
-- Pattern 72: Multiple new patterns combined
-- =============================================================================
SELECT
    id,
    OVERLAY(name PLACING '***' FROM 1 FOR 1) AS masked,
    created_at AT TIME ZONE 'UTC' AS utc_time,
    CEIL(score) AS rounded,
    tags[1] AS first_tag
FROM events
WHERE name IS DISTINCT FROM ''
  AND (score > 0) IS NOT UNKNOWN
ORDER BY name COLLATE "C";
