.PHONY: build test lint fmt clean release deny bench test-real test-acp

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

bench:
	cargo bench --workspace

deny:
	cargo deny check

# Run real-API integration tests (requires API keys set in the environment)
# Example: ANTHROPIC_API_KEY=sk-ant-... make test-real
test-real:
	cargo test --test real_api -- --ignored --nocapture

# Run ACP integration tests against a real ACP binary.
# Defaults to 'claude'; override with HONE_ACP_BIN=<binary>.
# Example: HONE_ACP_BIN=gemini-acp make test-acp
test-acp:
	HONE_ACP_BIN=$${HONE_ACP_BIN:-claude} cargo test -p hone-providers --test acp_real -- --ignored --nocapture

# Install locally
install:
	cargo install --path bins/hone
	cargo install --path bins/honed
