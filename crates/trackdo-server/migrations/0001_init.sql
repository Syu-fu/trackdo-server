CREATE TABLE users (
    id INTEGER PRIMARY KEY
);

-- Until users are managed, every token maps to this one.
INSERT INTO users (id) VALUES (1);

-- One row per stored operation. UUIDs are 16 big-endian bytes; u64 values
-- (HLC wall time, timestamps) are bit-cast to INTEGER.
CREATE TABLE ops (
    user_id     INTEGER NOT NULL REFERENCES users (id),
    seq         INTEGER NOT NULL CHECK (seq > 0),
    op_id       BLOB    NOT NULL CHECK (length(op_id) = 16),
    hlc_wall_ms INTEGER NOT NULL,
    hlc_counter INTEGER NOT NULL CHECK (hlc_counter BETWEEN 0 AND 4294967295),
    hlc_device  BLOB    NOT NULL CHECK (length(hlc_device) = 16),
    target      TEXT    NOT NULL,
    task_id     BLOB    NOT NULL CHECK (length(task_id) = 16),
    entry_id    BLOB             CHECK (length(entry_id) = 16),
    field       TEXT    NOT NULL,
    value_text  TEXT,
    -- A NULL stopped_at means the timer is running.
    value_int   INTEGER,
    PRIMARY KEY (user_id, seq),
    UNIQUE (user_id, op_id),
    CHECK (
        CASE target
            WHEN 'task' THEN entry_id IS NULL AND CASE field
                WHEN 'title' THEN value_text IS NOT NULL AND value_int IS NULL
                WHEN 'status' THEN value_text IN ('todo', 'done') AND value_int IS NULL
                WHEN 'deleted' THEN value_text IS NULL AND value_int IN (0, 1)
                ELSE 0
            END
            WHEN 'time_entry' THEN entry_id IS NOT NULL AND value_text IS NULL AND CASE field
                WHEN 'started_at' THEN value_int IS NOT NULL
                WHEN 'stopped_at' THEN 1
                WHEN 'deleted' THEN value_int IN (0, 1)
                ELSE 0
            END
            ELSE 0
        END
    )
) WITHOUT ROWID;

-- Finds the first stored op of a time entry, which fixes the entry's task.
CREATE INDEX ops_entry ON ops (user_id, entry_id, seq) WHERE entry_id IS NOT NULL;
