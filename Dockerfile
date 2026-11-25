FROM rust:1.85-slim-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    tmux git ca-certificates bash \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/hqcode /usr/local/bin/hqcode
EXPOSE 9900
WORKDIR /workspace
ENTRYPOINT ["hqcode", "serve", "--addr", "0.0.0.0:9900", "--workspace", "/workspace", "--data-dir", "/data"]
