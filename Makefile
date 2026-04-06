.PHONY: build test lint fmt clean release

build:
	cargo build

release:
	cargo build --release

test:
	cargo test --workspace

lint:
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

clean:
	cargo clean

check: fmt-check lint test

# Run the PTY integration tests (requires expect)
pty-test:
	expect tests/pty_tui_test.exp ./target/release/hone

# Install locally
install:
	cargo install --path bins/hone
	cargo install --path bins/honed
