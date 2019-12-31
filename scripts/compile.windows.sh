cargo install cargo-bloat
cargo build --release
cargo bloat --release
strip -s target/release/fluminurs.exe
cp target/release/fluminurs fluminurs.windows.exe
