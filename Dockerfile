FROM rust:1.75-bookworm AS builder

WORKDIR /app

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# Build release binary
ARG BIN_NAME
RUN cargo build --release --bin ${BIN_NAME}

# Runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

ARG BIN_NAME
COPY --from=builder /app/target/release/${BIN_NAME} /usr/local/bin/app

CMD ["/usr/local/bin/app"]
