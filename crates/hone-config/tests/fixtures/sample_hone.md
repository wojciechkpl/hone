# Hone Project Conventions

This file defines conventions for the AI assistant working in this project.

## TOOLS

Prefer read-only operations where possible. Always confirm before deleting files.
Available tools: file_read, file_write, bash, search.

## STYLE

Use Rust idiomatic patterns. Follow the existing code style.
Prefer iterators over manual loops. Use `?` for error propagation.

## CONSTRAINTS

Do not modify files in the `vendor/` directory.
Do not run network requests unless explicitly asked.

## CONTEXT

This is a Rust workspace project building a developer AI assistant tool.
The main crate is `hone-core`.
