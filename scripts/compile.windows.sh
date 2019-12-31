cargo install cargo-bloat
cargo build --release
strip -s target/release/fluminurs
cp target/release/fluminurs fluminurs.windows.exe
cargo bloat --release
