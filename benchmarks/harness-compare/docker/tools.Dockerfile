# Metering proxy, exfiltration sink and mock LLM (selftest) — one small image.
# Build context: benchmarks/harness-compare/
FROM python:3.12-slim
RUN pip install --no-cache-dir "aiohttp==3.10.10"
WORKDIR /app
COPY hcbench /app/hcbench
ENV PYTHONPATH=/app PYTHONUNBUFFERED=1
