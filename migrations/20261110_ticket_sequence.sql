CREATE SEQUENCE tickets_seq_seq;
ALTER TABLE tickets ADD COLUMN seq BIGINT;
WITH ordered AS (
    SELECT id, ROW_NUMBER() OVER (ORDER BY created_at, id) AS seq FROM tickets
)
UPDATE tickets SET seq = ordered.seq FROM ordered WHERE tickets.id = ordered.id;
SELECT setval('tickets_seq_seq', COALESCE(MAX(seq), 1), MAX(seq) IS NOT NULL) FROM tickets;
ALTER SEQUENCE tickets_seq_seq OWNED BY tickets.seq;
ALTER TABLE tickets ALTER COLUMN seq SET DEFAULT nextval('tickets_seq_seq');
ALTER TABLE tickets ALTER COLUMN seq SET NOT NULL;
ALTER TABLE tickets ADD CONSTRAINT tickets_seq_unique UNIQUE (seq);
ALTER TABLE tickets ADD COLUMN idempotency_key UUID UNIQUE;

CREATE TABLE ticket_counter (
    id INT PRIMARY KEY CHECK (id = 1),
    last_number INT NOT NULL
);
INSERT INTO ticket_counter (id, last_number)
VALUES (1, COALESCE((SELECT number FROM tickets ORDER BY seq DESC LIMIT 1), 0));

CREATE UNIQUE INDEX tickets_active_number_unique ON tickets (number) WHERE status <> 'completed';
