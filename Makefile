.PHONY: build test lint fmt clean release deny bench test-real test-acp coverage coverage-html coverage-ci webui webui-dev webui-clean

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

# Coverage measurement (requires: cargo install cargo-llvm-cov)
coverage:
	cargo llvm-cov --workspace --lcov --output-path lcov.info
	@echo "Coverage report: lcov.info"

coverage-html:
	cargo llvm-cov --workspace --html --output-dir coverage-html
	@echo "Open coverage-html/index.html"

coverage-ci:
	cargo llvm-cov --workspace --lcov --output-path lcov.info --fail-under-lines 60
	@echo "Coverage gate: 60% minimum passed"

# Install locally
install:
	cargo install --path bins/hone
	cargo install --path bins/honed

# Cross-session learning benchmark (10 mini tasks, 2 rounds)
# Requires: OPENAI_API_KEY, optional HONE_OPENAI_BASE_URL / HONE_OPENAI_MODEL
learning-benchmark: release
	python3 benchmarks/learning_benchmark.py

# -----------------------------------------------------------------------------
# Web UI (apps/webui — SolidJS + Vite + UnoCSS)
# -----------------------------------------------------------------------------
# Production build of the SPA. Output lands in apps/webui/dist/, which the
# hone-server crate picks up via rust-embed at compile time. After running
# `make webui` you must rebuild honed (`cargo build --release -p honed`) for
# the new bundle to be embedded in the binary.
webui:
	cd apps/webui && npm install --no-audit --no-fund && npm run build

# Run the SolidJS dev server with hot reload at http://127.0.0.1:5174.
# Vite proxies API + SSE requests to a locally-running honed at :3117.
webui-dev:
	cd apps/webui && npm install --no-audit --no-fund && npm run dev

webui-clean:
	rm -rf apps/webui/dist apps/webui/node_modules
