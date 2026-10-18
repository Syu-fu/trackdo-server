# trackdo-server design

The server validates, orders, stores and broadcasts operations. Conflict
resolution (LWW), timer healing and the "one running timer" rule all live in
the clients through [trackdo-core](https://github.com/Syu-fu/trackdo-core).
The server never holds a `Replica`; replaying the log to build a snapshot is
its only use of domain logic (`Task::apply` / `TimeEntry::apply`).

The wire format is `trackdo-protocol` v0.2.0 (`PROTOCOL_VERSION = 2`).

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

- Authenticated requests must send `Trackdo-Protocol: 2`. A missing or
  different value is rejected with `400 unsupported_protocol_version`.
  Authentication is checked first.
- Every response carries `Trackdo-Protocol: 2`, so a client can tell whether
  the app or the server needs updating.
- Only an exact match is accepted.

## API

### Errors

```json
{ "error": { "code": "task_mismatch", "message": "...", "op_id": "..." } }
```

`op_id` is present only when the error concerns a specific operation.

| Status | Code | When |
|---|---|---|
| 400 | `invalid_json` | Body is not a JSON array of `OpDto`, or not sent as `application/json` |
| 400 | `invalid_last_event_id` | `Last-Event-ID` is not a seq |
| 400 | `unsupported_protocol_version` | `Trackdo-Protocol` missing or different |
| 401 | `unauthorized` | Token missing or unknown (with `WWW-Authenticate: Bearer`) |
| 409 | `task_mismatch` | A time entry op names a different task than the entry's first stored op |
| 410 | `resync_required` | The event log cannot resume from `Last-Event-ID` |
| 413 | `too_large` | More than 500 ops or a body over 2 MiB |
| 422 | `invalid_op` | `TryFrom<OpDto> for Op` failed, e.g. a non-canonical title |
| 500 | `internal` | Anything else; details are logged, not returned |

### `GET /healthz`

Unauthenticated. Returns `{"status":"ok","protocol_version":2}` so apps can
check the URL and compatibility before asking for a token.

### `POST /ops`

- Body: a JSON array of `OpDto`, at most 500 items and 2 MiB.
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
- One event per op: `id: <seq>`, `event: op`, `data: <OpDto JSON>`. Clients
  pass each to `Replica::apply_remote(&op, seq)`; duplicates are harmless.
- A `:keepalive` comment line is sent every 15 seconds. No `retry:` field is sent;
  reconnecting with backoff is the client's job.
- When the stream cannot resume, the server answers `410 resync_required`
  before streaming. The client then fetches `GET /snapshot`, applies it, and
  reconnects with `Last-Event-ID` set to the snapshot's `seq`.
  Currently this happens only when `Last-Event-ID` is ahead of the newest
  `seq`, which means the server lost data.
- Delivery reads from the database only. The stream fetches up to 500 ops
  after the last one sent, and when caught up waits on a per-user notification
  that new ops were stored. It subscribes before reading, so no notification
  is missed, and since ops are never pushed through the channel there is no
  lag or gap between backlog and live delivery.
- Streams end on graceful shutdown so clients reconnect.

### `GET /snapshot`

Returns `SnapshotDto`: the log replayed in `seq` order with `Task::apply` /
`TimeEntry::apply`, with `seq` set to the newest `seq` (`null` for an empty
log). It is computed on every request; there is no cache.

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
  target      TEXT    NOT NULL,   -- 'task' | 'time_entry'
  task_id     BLOB    NOT NULL,
  entry_id    BLOB,               -- NOT NULL exactly for time entry ops
  field       TEXT    NOT NULL,
  value_text  TEXT,               -- title, status
  value_int   INTEGER,            -- times in ms, deleted as 0/1; NULL stop = running
  PRIMARY KEY (user_id, seq),
  UNIQUE (user_id, op_id),
  CHECK (...)                     -- which value column each target/field uses
) WITHOUT ROWID;

CREATE INDEX ops_entry ON ops (user_id, entry_id, seq) WHERE entry_id IS NOT NULL;
```

- `seq` is `MAX(seq) + 1` per user. Writes are serialized (`BEGIN IMMEDIATE`)
  so numbering never races.
- An entry's task is the `task_id` of its first stored op, looked up through
  `ops_entry`. Mismatches within a single batch are rejected too.
- `u64` values (HLC wall time, timestamps) are bit-cast to `i64`, which is
  lossless; SQL never orders by them.

### Not done (yet)

- **Log compaction.** An op is about 300 bytes, so the log stays small for
  personal use. When added, requests from before the compaction point get the
  same `410 resync_required`.
- **Recovery from server data loss.** After restoring an older database,
  clients hold a `last_seq` ahead of the log. `Replica::apply_snapshot` keeps
  `max(last_seq, snapshot.seq)`, so clients cannot rewind; fixing this needs a
  trackdo-core change (e.g. a log epoch). Until then the database must be
  backed up and never reset.

## Deployment and configuration

Exactly one instance with persistent storage: SQLite and the in-process
notification do not support more. Releases publish a container image to GHCR;
the hosting platform is not decided yet.

| Variable | Default | Meaning |
|---|---|---|
| `TRACKDO_LISTEN` | `127.0.0.1:8080` | Listen address (`0.0.0.0:8080` in containers) |
| `TRACKDO_DATABASE` | `trackdo.db` | SQLite file, created if missing |
| `TRACKDO_TOKEN_HASHES` | required | Comma-separated SHA-256 hex digests; startup fails if empty |
| `TRACKDO_CORS_ORIGINS` | empty (no CORS) | Comma-separated origins allowed for web clients |
| `RUST_LOG` | `info` | Log filter |

Generate a token and its digest with:

```sh
TOKEN=$(openssl rand -base64 32)
printf %s "$TOKEN" | shasum -a 256
```
