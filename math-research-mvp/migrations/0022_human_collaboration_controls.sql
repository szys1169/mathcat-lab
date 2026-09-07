-- Durable human-collaboration settings. The legacy boolean remains a
-- compatibility projection while review_mode preserves all three UI choices.
ALTER TABLE projects ADD COLUMN review_mode TEXT NOT NULL DEFAULT 'balanced'
    CHECK (review_mode IN ('automatic','balanced','strict'));

UPDATE projects
SET review_mode = CASE
    WHEN human_route_approval = 1 THEN 'strict'
    ELSE 'balanced'
END;
