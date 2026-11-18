# Metering proxy, exfiltration sink and mock LLM (selftest) — one small image.
# Build context: benchmarks/harness-compare/
FROM python:3.12-slim@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b
RUN pip install --no-cache-dir "aiohttp==3.10.10"
WORKDIR /app
COPY hcbench /app/hcbench
ENV PYTHONPATH=/app PYTHONUNBUFFERED=1
