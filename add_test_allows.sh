#!/usr/bin/env bash
set -euo pipefail

# Add #![allow(clippy::expect_used, clippy::unwrap_used)] to all integration test files

TEST_FILES=(
    crates/hone-providers/tests/acp_real.rs
    crates/hone-providers/tests/acp.rs
    crates/hone-providers/tests/anthropic_replay.rs
    crates/hone-providers/tests/anthropic.rs
    crates/hone-providers/tests/cli_providers.rs
    crates/hone-providers/tests/fallback.rs
    crates/hone-providers/tests/ollama_replay.rs
    crates/hone-providers/tests/ollama.rs
    crates/hone-providers/tests/openai_replay.rs
    crates/hone-providers/tests/openai.rs
    crates/hone-providers/tests/pricing.rs
    crates/hone-providers/tests/provider_contract.rs
    crates/hone-providers/tests/real_api.rs
    crates/hone-providers/tests/registry.rs
    crates/hone-providers/tests/router.rs
    crates/hone-cache/tests/memory_backend_tests.rs
    crates/hone-cache/tests/sqlite_backend_tests.rs
)

for file in "${TEST_FILES[@]}"; do
    if [ ! -f "$file" ]; then
        echo "Skipping $file (not found)"
        continue
    fi

    # Check if file already has the allow attribute
    if grep -q "^#!\[allow(clippy::expect_used" "$file" 2>/dev/null; then
        echo "Skipping $file (already has allow)"
        continue
    fi

    # Add the attribute as the first line
    echo "Adding allow to $file"
    tmpfile=$(mktemp)
    echo "#![allow(clippy::expect_used, clippy::unwrap_used)]" > "$tmpfile"
    echo "" >> "$tmpfile"
    cat "$file" >> "$tmpfile"
    mv "$tmpfile" "$file"
done

echo "Done!"
