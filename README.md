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

## Development

Requires the toolchain pinned in [`rust-toolchain.toml`](rust-toolchain.toml).

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo llvm-cov --workspace                   # coverage (CI requires >= 80%)
```

## License

[MIT](LICENSE)
