# harness-compare — see README.md and docs/plans/trust-defaults-daemon-2026-11.md
SHELL := /usr/bin/env bash
S := scripts
CAMPAIGN ?=
NAME ?=
PY := uv run --no-project -q --python 3.12 --with "aiohttp>=3.9"

.PHONY: help test lint check-claims selftest preflight bundle serve serve-down stack stack-down \
        smoke prereg run-a run-b run-c grade report package policy-gate

help:
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | sed 's/:.*## /\t/' | column -t -s $$'\t'

test: ## unit tests (336)
	$(PY) --with pytest python -m pytest -q
lint: ## ruff + shellcheck
	uv run --no-project -q --with ruff ruff check hcbench harness_bin tests
	uv run --no-project -q --with ruff ruff format --check hcbench harness_bin
	uv run --no-project -q --with shellcheck-py shellcheck -x -S warning $(S)/*.sh docker/redteam/plant.sh
check-claims: ## Phase 0: fail while withdrawn SWE-bench claims remain
	$(S)/check_claims.sh
selftest: ## no-GPU end-to-end plumbing check (mock LLM)
	$(S)/selftest.sh

preflight: ## P1.1 host checks
	$(S)/00_preflight.sh
bundle: ## P1.2 hone + pi bundle
	$(S)/01_build_bundle.sh
serve: ## P1.3 start hc-llm
	$(S)/02_serve_model.sh up
serve-down:
	$(S)/02_serve_model.sh down
stack: ## P1.4 proxy + sink + images
	$(S)/03_stack.sh up
stack-down:
	$(S)/03_stack.sh down
smoke: ## P1.5 capability probes
	$(S)/04_smoke.sh
prereg: ## P1.6 pre-register (NAME=...)
	@test -n "$(NAME)" || (echo "NAME=... required" && exit 2)
	$(S)/05_preregister.sh $(NAME)
run-a: ## P1.8 suite A (CAMPAIGN=...)
	@test -n "$(CAMPAIGN)" || (echo "CAMPAIGN=... required" && exit 2)
	$(S)/10_run.sh a $(CAMPAIGN)
run-b: ## P1.9 suite B (CAMPAIGN=...)
	@test -n "$(CAMPAIGN)" || (echo "CAMPAIGN=... required" && exit 2)
	$(S)/10_run.sh b $(CAMPAIGN)
run-c: ## P1.7 suite C (CAMPAIGN=...)
	@test -n "$(CAMPAIGN)" || (echo "CAMPAIGN=... required" && exit 2)
	$(S)/10_run.sh c $(CAMPAIGN)
grade: ## P1.10 official swebench grading (CAMPAIGN=...)
	$(S)/20_grade.sh $(CAMPAIGN)
report: ## P1.11 report.md / runs.csv / redteam.csv (CAMPAIGN=...)
	$(S)/30_report.sh $(CAMPAIGN)
package: ## P1.11 publication bundle (CAMPAIGN=...)
	$(S)/30_report.sh $(CAMPAIGN) --package
policy-gate: ## P2.8 suite C + B on a Hone branch (CAMPAIGN=..., HC_HONE_REF=...)
	$(S)/40_policy_gate.sh $(CAMPAIGN)
