#!/bin/sh
cargo install cargo-bloat
cargo build --release
cargo bloat --release
strip target/release/fluminurs
cp target/release/fluminurs fluminurs.macos
