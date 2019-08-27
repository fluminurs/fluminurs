#!/bin/sh
cargo build --release
strip target/release/fluminurs
cp target/release/fluminurs fluminurs.macos
