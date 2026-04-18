#!/usr/bin/env bash
# Ollama integration demo for asciinema.
# Shows local model discovery and a real agent turn end-to-end.
set -e
BIN=./target/release/hone

echo "# 1. Local Ollama is running with a small coder model:"
echo "$ ollama list"
ollama list
echo
sleep 2

echo "# 2. Hone auto-detects Ollama when no other provider is configured:"
echo "$ curl -s http://localhost:11434/api/tags | jq '.models[].name'"
curl -s http://localhost:11434/api/tags | jq '.models[].name'
echo
sleep 2

echo "# 3. Run a single agent turn against the local model."
echo "#    (--no-clarify skips the interactive clarifier so the cast stays deterministic)"
echo "$ hone --no-clarify --model ollama:qwen2.5-coder:1.5b \\"
echo "       'Write a one-line Rust closure that doubles an int.'"
$BIN --no-clarify --model ollama:qwen2.5-coder:1.5b \
    'Write a one-line Rust closure that doubles an int.'
echo
sleep 2

echo "# 4. For tool-capable turns, pull a larger model and retry:"
echo "#   ollama pull qwen2.5-coder:14b"
echo "#   hone --model ollama:qwen2.5-coder:14b 'refactor the cache module'"
