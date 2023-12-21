# Build Stage

FROM rust:1.74.0 AS builder
WORKDIR /root/workdir

COPY Cargo.toml Cargo.lock .
RUN \
    mkdir /root/workdir/src && \
    echo 'fn main() {}' > /root/workdir/src/main.rs && \
    cargo build --release && \
    rm -Rvf /root/workdir/src

COPY src ./src
RUN cargo build --release

# Bundle Stage

FROM ubuntu:23.04
COPY --from=builder /root/workdir/target/*/release/air-remote-mediator .
CMD ["./air-remote-mediator"]
