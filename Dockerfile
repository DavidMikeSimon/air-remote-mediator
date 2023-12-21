# Build Stage

FROM rust:1.74.0 AS builder
WORKDIR /root/workdir

COPY Cargo.toml Cargo.lock .
RUN \
    mkdir /root/workdir/src && \
    echo 'fn main() {}' > /root/workdir/src/main.rs && \
    cargo build --release && \
    rm -Rvf /root/workdir/src && \
    rm -Rvf /root/workdir/target/release/deps/air_remote_mediator*

COPY src ./src
RUN cargo build --release

# Bundle Stage

FROM debian:bookworm-20231218-slim
COPY --from=builder /root/workdir/target/release/air-remote-mediator .
CMD ["./air-remote-mediator"]
