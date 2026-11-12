ALTER TABLE tickets ADD COLUMN seated_count INT NOT NULL DEFAULT 0;
ALTER TABLE tickets ADD CONSTRAINT tickets_seated_count_range CHECK (seated_count >= 0 AND seated_count <= group_size);
