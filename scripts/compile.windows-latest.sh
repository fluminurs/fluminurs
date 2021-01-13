cargo install cargo-bloat
cargo build --release
cargo bloat --release
strip -s target/release/fluminurs-cli.exe
cp target/release/fluminurs-cli fluminurs-cli.windows.exe
