#!/bin/sh
cargo build --release
strip -s target/release/fluminurs
cp target/release/fluminurs fluminurs.linux
