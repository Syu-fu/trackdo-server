# trackdo-server

Sync server for **trackdo**, a personal app that combines a to-do list with
time tracking. It validates, orders, stores and broadcasts the operations
defined in [trackdo-core](https://github.com/Syu-fu/trackdo-core); conflict
resolution stays on the clients.

> **Status:** early development.

See [docs/design.md](docs/design.md) for the API, storage and deployment.

## Running

```sh
TOKEN=$(openssl rand -base64 32)          # give this to the apps
export TRACKDO_TOKEN_HASHES=$(printf %s "$TOKEN" | shasum -a 256 | cut -d' ' -f1)
cargo run --release
```

Other settings (listen address, database path, log filter) are listed in
[docs/design.md](docs/design.md#deployment-and-configuration).

### Container

Pushing a `v*` tag publishes `ghcr.io/syu-fu/trackdo-server` (linux/amd64).
Run exactly one instance, with `/data` on persistent storage:

```sh
docker run -d -p 8080:8080 -v trackdo:/data \
  -e TRACKDO_TOKEN_HASHES="$TRACKDO_TOKEN_HASHES" \
  ghcr.io/syu-fu/trackdo-server:0.1
```

Put TLS in front (a reverse proxy or tunnel), point health checks at
`GET /healthz`, and back up `/data`: the server cannot recover clients from a
lost or rolled-back database (see [docs/design.md](docs/design.md#not-done-yet)).

## Development

Requires the toolchain pinned in [`rust-toolchain.toml`](rust-toolchain.toml).

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo llvm-cov --workspace                   # coverage (CI requires >= 80%)
```

## License

[MIT](LICENSE)
