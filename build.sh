#!/bin/sh
cargo zigbuild --target aarch64-unknown-linux-gnu --features hotpath,hotpath-alloc --release && scp target/aarch64-unknown-linux-gnu/release/air-remote-mediator dsimon@terminal1:~

