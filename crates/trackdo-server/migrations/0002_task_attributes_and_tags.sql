-- trackdo-core v0.4.0: task attributes, subtasks and tags. SQLite cannot
-- change a CHECK constraint in place, so the table is rebuilt.
CREATE TABLE ops_v2 (
    user_id     INTEGER NOT NULL REFERENCES users (id),
    seq         INTEGER NOT NULL CHECK (seq > 0),
    op_id       BLOB    NOT NULL CHECK (length(op_id) = 16),
    hlc_wall_ms INTEGER NOT NULL,
    hlc_counter INTEGER NOT NULL CHECK (hlc_counter BETWEEN 0 AND 4294967295),
    hlc_device  BLOB    NOT NULL CHECK (length(hlc_device) = 16),
    target      TEXT    NOT NULL,
    -- The entity written: task_id for tasks and time entries, entry_id for
    -- time entries, tag_id for tags and for a task's tag.
    task_id     BLOB             CHECK (length(task_id) = 16),
    entry_id    BLOB             CHECK (length(entry_id) = 16),
    tag_id      BLOB             CHECK (length(tag_id) = 16),
    field       TEXT    NOT NULL,
    -- The value written. NULL clears due, scheduled_at and parent, and a
    -- NULL stopped_at means the timer is running.
    value_text  TEXT,
    value_int   INTEGER,
    value_id    BLOB             CHECK (length(value_id) = 16),
    PRIMARY KEY (user_id, seq),
    UNIQUE (user_id, op_id),
    -- A NULL comparison would pass the check, so NULL counts as false.
    CHECK (coalesce(
        CASE target
            WHEN 'task' THEN task_id IS NOT NULL AND entry_id IS NULL
                AND (tag_id IS NOT NULL) = (field = 'tag')
                AND (value_id IS NULL OR field = 'parent')
                AND CASE field
                    WHEN 'title' THEN value_text IS NOT NULL AND value_int IS NULL
                    WHEN 'status' THEN value_text IN ('todo', 'done') AND value_int IS NULL
                    WHEN 'deleted' THEN value_text IS NULL AND value_int IN (0, 1)
                    WHEN 'priority' THEN value_text IN ('none', 'low', 'medium', 'high')
                        AND value_int IS NULL
                    WHEN 'note' THEN value_text IS NOT NULL AND value_int IS NULL
                    WHEN 'due' THEN (value_text IS NULL OR date(value_text) IS value_text)
                        AND value_int IS NULL
                    WHEN 'scheduled_at' THEN value_text IS NULL
                    WHEN 'parent' THEN value_text IS NULL AND value_int IS NULL
                    WHEN 'tag' THEN value_text IS NULL AND value_int IN (0, 1)
                    ELSE 0
                END
            WHEN 'time_entry' THEN task_id IS NOT NULL AND entry_id IS NOT NULL
                AND tag_id IS NULL AND value_id IS NULL AND value_text IS NULL
                AND CASE field
                    WHEN 'started_at' THEN value_int IS NOT NULL
                    WHEN 'stopped_at' THEN 1
                    WHEN 'deleted' THEN value_int IN (0, 1)
                    ELSE 0
                END
            WHEN 'tag' THEN task_id IS NULL AND entry_id IS NULL AND tag_id IS NOT NULL
                AND value_id IS NULL
                AND CASE field
                    WHEN 'name' THEN value_text IS NOT NULL AND value_int IS NULL
                    WHEN 'deleted' THEN value_text IS NULL AND value_int IN (0, 1)
                    ELSE 0
                END
            ELSE 0
        END,
        0
    ))
) WITHOUT ROWID;

INSERT INTO ops_v2 (user_id, seq, op_id, hlc_wall_ms, hlc_counter, hlc_device,
                    target, task_id, entry_id, field, value_text, value_int)
SELECT user_id, seq, op_id, hlc_wall_ms, hlc_counter, hlc_device,
       target, task_id, entry_id, field, value_text, value_int
FROM ops;

DROP TABLE ops;
ALTER TABLE ops_v2 RENAME TO ops;

-- Finds the first stored op of a time entry, which fixes the entry's task.
CREATE INDEX ops_entry ON ops (user_id, entry_id, seq) WHERE entry_id IS NOT NULL;
