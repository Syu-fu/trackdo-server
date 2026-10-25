# trackdo-server design

The server validates, orders, stores and broadcasts operations. Conflict
resolution (LWW), timer healing and the "one running timer" rule all live in
the clients through [trackdo-core](https://github.com/Syu-fu/trackdo-core).
The server never holds a `Replica`; replaying the log to build a snapshot is
its only use of domain logic (`Task::apply` / `TimeEntry::apply` /
`Tag::apply`).

The wire format is `trackdo-protocol` v0.4.0 (`PROTOCOL_VERSION = 4`).
Subtask trees, cycle breaking and hidden deleted tags are derived on the
clients too; the server stores and replays task, time entry and tag ops alike.

## Authentication

- Every endpoint except `GET /healthz` requires `Authorization: Bearer <token>`,
  including the event stream. Web clients read SSE through `fetch` streaming
  rather than `EventSource`, which cannot set headers.
- Tokens are static. `TRACKDO_TOKEN_HASHES` lists their SHA-256 digests (hex,
  comma-separated), typically one per device. Tokens are compared by digest in
  constant time and never stored in plain text.
- Clients never send a user id. Every token currently maps to the single
  default user (`users.id = 1`); the schema is already keyed by user so tokens
  can move to a table when more users are needed.
- Apps are configured with the server URL and a token.

## Protocol version

- Authenticated requests must send `Trackdo-Protocol: 4`. A missing or
  different value is rejected with `400 unsupported_protocol_version`.
  Authentication is checked first.
- Every response carries `Trackdo-Protocol: 4`, so a client can tell whether
  the app or the server needs updating.
- Only an exact match is accepted.

## Log identity

Each user's op log is served under a `log_id` (a UUID), which `GET /snapshot`
returns and every event id carries. Clients resume `GET /events` only within
the log they know; a position in another log is refused, so they resync.

- **A new random `log_id` at every server start.** It is not stored: restoring
  a backup restores the database, so a stored id would come back with the old
  log and the rollback would go unnoticed. Restoring always means a restart,
  so it is always caught without operator action.
- The id stays the same for the life of a process, so a snapshot and the
  events after it always agree. One id serves every user's log, since ids are
  only compared within one user's stream.
- **Cost:** after every restart each client fetches one snapshot. A replica
  re-sends nothing unless the log actually lost writes; if it did, the replica
  queues the missing writes again with their original HLCs
  (`Replica::apply_snapshot`, trackdo-core v0.3.0), and the server stores them
  like any other ops.

## API

### Errors

```json
{ "error": { "code": "task_mismatch", "message": "...", "op_id": "..." } }
```

`op_id` is present only when the error concerns a specific operation.

| Status | Code | When |
|---|---|---|
| 400 | `invalid_json` | Body is not a JSON array of `OpDto`, or not sent as `application/json` |
| 400 | `invalid_last_event_id` | `Last-Event-ID` is not `<log_id>:<seq>` |
| 400 | `unsupported_protocol_version` | `Trackdo-Protocol` missing or different |
| 401 | `unauthorized` | Token missing or unknown (with `WWW-Authenticate: Bearer`) |
| 409 | `task_mismatch` | A time entry op names a different task than the entry's first stored op |
| 410 | `resync_required` | `Last-Event-ID` names another log, or a seq ahead of this one |
| 413 | `too_large` | More than 500 ops or a body over 2 MiB |
| 422 | `invalid_op` | `TryFrom<OpDto> for Op` failed, e.g. a non-canonical title |
| 500 | `internal` | Anything else; details are logged, not returned |

### `GET /healthz`

Unauthenticated. Returns `{"status":"ok","protocol_version":4}` so apps can
check the URL and compatibility before asking for a token.

### `POST /ops`

- Body: a JSON array of `OpDto`, at most 500 items and 2 MiB. Notes can be
  up to 10,000 characters, so clients split batches by size as well as by
  count.
- The batch is atomic: if any op is invalid or mismatches its entry's task,
  nothing is stored and the error names the first offending `op_id`.
- Ops are deduplicated by `op_id` per user. Retransmitted ops are not stored
  again and are not re-checked, since they were validated when first stored.
- New ops get a per-user, strictly increasing `seq` in arrival order.
- Response: `200 {"accepted": [<op_id>, ...]}` listing every op of the
  request, retransmissions included. Clients pass it to
  `Replica::acknowledge`.

### `GET /events`

Server-sent events of every op after a position, in `seq` order.

- The position is the `Last-Event-ID` header, used both for the first
  connection and for reconnects. Without it the stream starts at the beginning
  of the log.
- One event per op: `id: <log_id>:<seq>` (`trackdo_protocol::event::EventId`),
  `event: op`, `data: <OpDto JSON>`. Clients pass each op with its `seq` to
  `Replica::apply_remote`; duplicates are harmless.
- A `:keepalive` comment line is sent every 15 seconds. No `retry:` field is sent;
  reconnecting with backoff is the client's job.
- When the stream cannot resume, the server answers `410 resync_required`
  before streaming. The client then fetches `GET /snapshot`, applies it, and
  reconnects from the snapshot's `log_id` and `seq`. This happens when
  `Last-Event-ID` names another log (the server restarted, perhaps from a
  backup) or a `seq` ahead of the newest one (the log lost ops).
- Delivery reads from the database only. The stream fetches up to 500 ops
  after the last one sent, and when caught up waits on a per-user notification
  that new ops were stored. It subscribes before reading, so no notification
  is missed, and since ops are never pushed through the channel there is no
  lag or gap between backlog and live delivery.
- Streams end on graceful shutdown so clients reconnect.

### `GET /snapshot`

Returns `SnapshotDto`: the log replayed in `seq` order with `Task::apply` /
`TimeEntry::apply` / `Tag::apply`, with the log's `log_id` and `seq` set to
the newest `seq` (`null` for an empty log). It is computed on every request;
there is no cache.

## Storage

SQLite in WAL mode through sqlx, using runtime queries (no compile-time
macros) and embedded migrations (`sqlx::migrate!`).

```sql
CREATE TABLE users (id INTEGER PRIMARY KEY);

CREATE TABLE ops (
  user_id     INTEGER NOT NULL REFERENCES users(id),
  seq         INTEGER NOT NULL,
  op_id       BLOB    NOT NULL,   -- UUID, 16 bytes
  hlc_wall_ms INTEGER NOT NULL,
  hlc_counter INTEGER NOT NULL,
  hlc_device  BLOB    NOT NULL,
  target      TEXT    NOT NULL,   -- 'task' | 'time_entry' | 'tag'
  task_id     BLOB,               -- task and time entry ops
  entry_id    BLOB,               -- time entry ops
  tag_id      BLOB,               -- tag ops, and the tag of a task 'tag' op
  field       TEXT    NOT NULL,
  value_text  TEXT,               -- title, status, priority, note, due, tag name
  value_int   INTEGER,            -- times in ms, flags as 0/1; NULL stop = running
  value_id    BLOB,               -- parent; NULL = top level
  PRIMARY KEY (user_id, seq),
  UNIQUE (user_id, op_id),
  CHECK (...)                     -- which id and value columns each target/field uses
) WITHOUT ROWID;

CREATE INDEX ops_entry ON ops (user_id, entry_id, seq) WHERE entry_id IS NOT NULL;
```

- `seq` is `MAX(seq) + 1` per user. Writes are serialized (`BEGIN IMMEDIATE`)
  so numbering never races.
- An entry's task is the `task_id` of its first stored op, looked up through
  `ops_entry`. Mismatches within a single batch are rejected too.
- `u64` values (HLC wall time, timestamps) are bit-cast to `i64`, which is
  lossless; SQL never orders by them. Due dates are stored as `YYYY-MM-DD`.
- The `CHECK` treats a `NULL` result as a failure, so a missing value never
  slips through a comparison.
- Rows are decoded back through the core types (`Title`, `Note`, `TagName`,
  `Date`), and a stored value those would change is reported as corrupt
  rather than served.
- Migration `0002` rebuilt `ops` for trackdo-core v0.4.0, since SQLite cannot
  change a `CHECK` in place, and copied every existing row unchanged.

### Not done (yet)

- **Log compaction.** An op is about 300 bytes, so the log stays small for
  personal use. When added, requests from before the compaction point get the
  same `410 resync_required`.

## Deployment and configuration

Exactly one instance with persistent storage: SQLite and the in-process
notification do not support more. Releases publish a container image to GHCR;
the hosting platform is not decided yet.

| Variable | Default | Meaning |
|---|---|---|
| `TRACKDO_LISTEN` | `127.0.0.1:8080` | Listen address (`0.0.0.0:8080` in containers) |
| `TRACKDO_DATABASE` | `trackdo.db` | SQLite file, created if missing |
| `TRACKDO_TOKEN_HASHES` | required | Comma-separated SHA-256 hex digests; startup fails if empty |
| `TRACKDO_CORS_ORIGINS` | empty (no CORS) | Comma-separated origins allowed for web clients, exactly as browsers send them (`https://app.example`, no path) |
| `RUST_LOG` | `info` | Log filter |

Generate a token and its digest with:

```sh
TOKEN=$(openssl rand -base64 32)
printf %s "$TOKEN" | shasum -a 256
```
