#!/bin/sh
cargo install cargo-bloat
cargo build --release
cargo bloat --release
strip -s target/release/fluminurs-cli
cp target/release/fluminurs-cli fluminurs-cli.ubuntu
