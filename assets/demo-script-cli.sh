#!/usr/bin/env bash
# Non-interactive CLI feature demo for asciinema.
set -e
BIN=./target/release/hone

echo "$ hone --version"
$BIN --version
echo
sleep 1

echo "$ hone --list-recipes"
$BIN --list-recipes 2>/dev/null | head -12
echo
sleep 2

echo "$ hone reports list --limit 3"
$BIN reports list --limit 3
echo
sleep 1

echo "$ hone --help | head -20"
$BIN --help 2>/dev/null | head -20
