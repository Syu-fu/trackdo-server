# syntax=docker/dockerfile:1

FROM rust:1.87.0-slim-bookworm@sha256:437507c3e719e4f968033b88d851ffa9f5aceeb2dcc2482cc6cb7647811a55eb AS build
# Use the image's toolchain; rust-toolchain.toml would also fetch dev-only
# components.
ENV RUSTUP_TOOLCHAIN=1.87.0
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p trackdo-server \
    && cp target/release/trackdo-server /usr/local/bin/trackdo-server
# Distroless has no shell to create the volume directory with.
RUN mkdir /data

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f
COPY --from=build /usr/local/bin/trackdo-server /usr/local/bin/trackdo-server
COPY --from=build --chown=nonroot:nonroot /data /data
ENV TRACKDO_LISTEN=0.0.0.0:8080 \
    TRACKDO_DATABASE=/data/trackdo.db
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/trackdo-server"]
