-- Legacy ttft_ms measures first bytes; do not backfill it as first content.
ALTER TABLE requests ADD COLUMN first_content_ms INTEGER;
