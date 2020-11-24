#!/bin/sh
cargo install cargo-bloat
cargo build --release
cargo bloat --release
strip -s target/release/fluminurs
cp target/release/fluminurs fluminurs.ubuntu-latest
