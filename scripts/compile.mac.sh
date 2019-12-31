#!/bin/sh
cargo install cargo-bloat
cargo build --release
strip target/release/fluminurs
cp target/release/fluminurs fluminurs.macos
cargo bloat --release
