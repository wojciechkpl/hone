# Suite B / smoke base: Python + git + pytest. Harness binaries come from the
# read-only bundle mounted at /opt/harness.
FROM python:3.12-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends git ca-certificates procps \
 && rm -rf /var/lib/apt/lists/* \
 && pip install --no-cache-dir "pytest==8.3.3"
RUN git config --system user.email bench@hcbench.local \
 && git config --system user.name hcbench \
 && git config --system init.defaultBranch main \
 && git config --system --add safe.directory '*'
WORKDIR /work
