#!/bin/sh
cargo install cargo-bloat
cargo build --release --bin fluminurs-cli --features="cli"
cargo bloat --release --bin fluminurs-cli --features="cli"
strip target/release/fluminurs-cli
cp target/release/fluminurs-cli fluminurs-cli.macos
