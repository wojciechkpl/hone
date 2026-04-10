# benchmarks/Makefile — Convenience targets for the Hone benchmark harness
#
# Run from the repository root:
#   make -f benchmarks/Makefile dry-run
#   make -f benchmarks/Makefile ablation
#
# Or from the benchmarks/ directory directly:
#   make dry-run

SHELL := /usr/bin/env bash
.SHELLFLAGS := -euo pipefail -c

REPO_ROOT := $(shell git rev-parse --show-toplevel 2>/dev/null || pwd)
BENCH_DIR := $(REPO_ROOT)/benchmarks
RESULTS_DIR := $(BENCH_DIR)/results

# ---------------------------------------------------------------------------
# Model / provider configuration
# These can be overridden on the command line:
#   make ablation HONE_OPENAI_MODEL=deepseek/deepseek-v3 HONE_OPENAI_BASE_URL=https://openrouter.ai/api
# ---------------------------------------------------------------------------

HONE_OPENAI_BASE_URL ?= https://api.deepseek.com
HONE_OPENAI_MODEL    ?= deepseek-chat
HONE_BENCH_USD_CAP   ?= 25
TASK_LIMIT           ?= 225
HONE_BIN             ?= $(REPO_ROOT)/target/release/hone

export HONE_OPENAI_BASE_URL
export HONE_OPENAI_MODEL
export HONE_BENCH_USD_CAP
export HONE_BIN
export RESULTS_DIR

# ---------------------------------------------------------------------------
# Phony targets
# ---------------------------------------------------------------------------

.PHONY: help build dry-run ablation ablation-resume headline analyze \
        analyze-chart clean-results check-env

help: ## Show this help message
	@echo ""
	@echo "Hone Benchmark Harness"
	@echo "======================"
	@echo ""
	@echo "USAGE"
	@echo "  make -f benchmarks/Makefile <target> [VAR=value ...]"
	@echo ""
	@echo "TARGETS"
	@grep -E '^[a-zA-Z_-]+:.*##' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*##"}; {printf "  %-18s %s\n", $$1, $$2}'
	@echo ""
	@echo "OVERRIDEABLE VARIABLES"
	@echo "  HONE_OPENAI_BASE_URL  Provider base URL  (default: https://api.deepseek.com)"
	@echo "  HONE_OPENAI_MODEL     Model name         (default: deepseek-chat)"
	@echo "  HONE_BENCH_USD_CAP    USD cap per config (default: 25)"
	@echo "  TASK_LIMIT            Tasks per config   (default: 225)"
	@echo "  HONE_BIN              Hone binary path   (default: ./target/release/hone)"
	@echo ""
	@echo "ENVIRONMENT VARIABLES (must be exported before make)"
	@echo "  OPENAI_API_KEY        Required for ablation runs (DeepSeek / OpenRouter)"
	@echo "  ANTHROPIC_API_KEY     Required for headline run (Claude Sonnet)"
	@echo ""
	@echo "EXAMPLES"
	@echo "  # Dry-run all ablation configs to verify setup"
	@echo "  make -f benchmarks/Makefile dry-run"
	@echo ""
	@echo "  # Run ablations via OpenRouter with a single key"
	@echo "  OPENAI_API_KEY=sk-or-... \\"
	@echo "  make -f benchmarks/Makefile ablation \\"
	@echo "    HONE_OPENAI_BASE_URL=https://openrouter.ai/api \\"
	@echo "    HONE_OPENAI_MODEL=deepseek/deepseek-v3"
	@echo ""
	@echo "  # Run just the A0 and A6 validation pair"
	@echo "  make -f benchmarks/Makefile ablation ABLATION_CONFIG=a0"
	@echo "  make -f benchmarks/Makefile ablation ABLATION_CONFIG=a6"
	@echo ""
	@echo "  # Headline run (requires ANTHROPIC_API_KEY)"
	@echo "  make -f benchmarks/Makefile headline"

# ---------------------------------------------------------------------------

build: ## Build hone release binary (--locked, reproducible)
	@echo "[make] Building hone release binary..."
	cd $(REPO_ROOT) && cargo build --release --locked
	@echo "[make] Binary: $(HONE_BIN)"
	$(HONE_BIN) --version

check-env: ## Verify required env vars and dependencies are in place
	@echo "[make] Checking environment..."
	@if [ -z "$$OPENAI_API_KEY" ] && [ -z "$$ANTHROPIC_API_KEY" ]; then \
		echo "ERROR: Neither OPENAI_API_KEY nor ANTHROPIC_API_KEY is set."; \
		exit 1; \
	fi
	@if [ ! -f "$(HONE_BIN)" ]; then \
		echo "ERROR: Hone binary not found at $(HONE_BIN). Run: make build"; \
		exit 1; \
	fi
	@echo "[make] OK — provider key set, binary found at $(HONE_BIN)"
	@$(HONE_BIN) --version

# ---------------------------------------------------------------------------

dry-run: ## Dry-run all 9 ablation configs (prints commands, does not execute)
	@echo "[make] Dry-run: ablation configs A0-A8"
	bash $(BENCH_DIR)/run_ablation.sh --dry-run --limit 5

dry-run-headline: ## Dry-run the SWE-bench Verified headline configuration
	@echo "[make] Dry-run: SWE-bench Verified headline run"
	bash $(BENCH_DIR)/run_headline.sh --dry-run --limit 5

# ---------------------------------------------------------------------------

ablation: check-env ## Run all 9 ablation configs (A0-A8) on Aider polyglot
	@echo "[make] Running ablation suite..."
	@echo "  Model:      $(HONE_OPENAI_MODEL)"
	@echo "  Base URL:   $(HONE_OPENAI_BASE_URL)"
	@echo "  Tasks:      $(TASK_LIMIT)"
	@echo "  USD cap:    $$$(HONE_BENCH_USD_CAP) per config"
	@echo ""
	bash $(BENCH_DIR)/run_ablation.sh \
		$(if $(ABLATION_CONFIG),--config $(ABLATION_CONFIG)) \
		--limit $(TASK_LIMIT)

ablation-resume: check-env ## Resume ablation run (skips already-completed configs)
	@echo "[make] Resuming ablation suite (skipping completed configs)..."
	bash $(BENCH_DIR)/run_ablation.sh \
		$(if $(ABLATION_CONFIG),--config $(ABLATION_CONFIG)) \
		--limit $(TASK_LIMIT) \
		--resume

# ---------------------------------------------------------------------------

headline: ## Run the headline SWE-bench Verified run on Claude Sonnet 4.5
	@echo "[make] Running headline SWE-bench Verified run..."
	@if [ -z "$$ANTHROPIC_API_KEY" ]; then \
		echo "ERROR: ANTHROPIC_API_KEY is required for the headline run."; \
		exit 1; \
	fi
	bash $(BENCH_DIR)/run_headline.sh $(if $(LIMIT),--limit $(LIMIT))

headline-resume: ## Resume a partially-completed headline run
	bash $(BENCH_DIR)/run_headline.sh --resume $(if $(LIMIT),--limit $(LIMIT))

# ---------------------------------------------------------------------------

analyze: ## Show comparison table for all results in benchmarks/results/
	python3 $(BENCH_DIR)/analyze.py \
		--results-dir $(RESULTS_DIR) \
		$(if $(RUN_ID),--run-id $(RUN_ID)) \
		$(if $(BENCHMARK),--benchmark $(BENCHMARK))

analyze-chart: ## Generate PNG charts (requires: pip install matplotlib)
	python3 $(BENCH_DIR)/analyze.py \
		--results-dir $(RESULTS_DIR) \
		--chart \
		--chart-dir $(RESULTS_DIR)/charts

analyze-csv: ## Output all results as CSV
	python3 $(BENCH_DIR)/analyze.py \
		--results-dir $(RESULTS_DIR) \
		--format csv

analyze-ci: ## Show results with 95% bootstrap confidence intervals (slow)
	python3 $(BENCH_DIR)/analyze.py \
		--results-dir $(RESULTS_DIR) \
		--ci

# ---------------------------------------------------------------------------

# ---------------------------------------------------------------------------
# SWE-bench targets
# ---------------------------------------------------------------------------

PYTHON := $(BENCH_DIR)/.venv/bin/python
SWEBENCH_DATASET ?= princeton-nlp/SWE-bench_Lite
SWEBENCH_LIMIT ?=
SWEBENCH_TIMEOUT ?= 300
SWEBENCH_WORKERS ?= 2

swebench-dry-run: build ## Run SWE-bench inference on 3 tasks (quick validation)
	OPENAI_API_KEY=$(OPENAI_API_KEY) \
	HONE_OPENAI_BASE_URL=$(HONE_OPENAI_BASE_URL) \
	HONE_OPENAI_MODEL=$(HONE_OPENAI_MODEL) \
		$(PYTHON) $(BENCH_DIR)/swebench_inference.py \
			--dataset $(SWEBENCH_DATASET) \
			--model $(HONE_OPENAI_MODEL) \
			--limit 3 \
			--timeout $(SWEBENCH_TIMEOUT)

swebench-lite: build ## Run full SWE-bench Lite (300 tasks)
	OPENAI_API_KEY=$(OPENAI_API_KEY) \
	HONE_OPENAI_BASE_URL=$(HONE_OPENAI_BASE_URL) \
	HONE_OPENAI_MODEL=$(HONE_OPENAI_MODEL) \
		$(PYTHON) $(BENCH_DIR)/swebench_inference.py \
			--dataset princeton-nlp/SWE-bench_Lite \
			--model $(HONE_OPENAI_MODEL) \
			$(if $(SWEBENCH_LIMIT),--limit $(SWEBENCH_LIMIT)) \
			--timeout $(SWEBENCH_TIMEOUT)

swebench-verified: build ## Run full SWE-bench Verified (500 tasks, headline run)
	OPENAI_API_KEY=$(OPENAI_API_KEY) \
	HONE_OPENAI_BASE_URL=$(HONE_OPENAI_BASE_URL) \
	HONE_OPENAI_MODEL=$(HONE_OPENAI_MODEL) \
		$(PYTHON) $(BENCH_DIR)/swebench_inference.py \
			--dataset princeton-nlp/SWE-bench_Verified \
			--model $(HONE_OPENAI_MODEL) \
			$(if $(SWEBENCH_LIMIT),--limit $(SWEBENCH_LIMIT)) \
			--timeout $(SWEBENCH_TIMEOUT)

swebench-resume: build ## Resume an interrupted SWE-bench run
	OPENAI_API_KEY=$(OPENAI_API_KEY) \
	HONE_OPENAI_BASE_URL=$(HONE_OPENAI_BASE_URL) \
	HONE_OPENAI_MODEL=$(HONE_OPENAI_MODEL) \
		$(PYTHON) $(BENCH_DIR)/swebench_inference.py \
			--dataset $(SWEBENCH_DATASET) \
			--model $(HONE_OPENAI_MODEL) \
			--resume \
			--timeout $(SWEBENCH_TIMEOUT)

swebench-evaluate: ## Evaluate predictions (requires Docker)
	@if [ -z "$(PREDICTIONS)" ]; then \
		echo "Usage: make swebench-evaluate PREDICTIONS=path/to/predictions.jsonl"; \
		exit 1; \
	fi
	$(PYTHON) $(BENCH_DIR)/swebench_evaluate.py \
		--predictions $(PREDICTIONS) \
		--dataset $(SWEBENCH_DATASET) \
		--max-workers $(SWEBENCH_WORKERS)

# ---------------------------------------------------------------------------

clean-results: ## Remove all result directories (DESTRUCTIVE — prompts for confirmation)
	@echo "WARNING: This will delete all benchmark results under $(RESULTS_DIR)/"
	@read -p "Type 'yes' to confirm: " confirm; \
	if [ "$$confirm" = "yes" ]; then \
		rm -rf $(RESULTS_DIR)/*/; \
		echo "Results cleared."; \
	else \
		echo "Aborted."; \
	fi
