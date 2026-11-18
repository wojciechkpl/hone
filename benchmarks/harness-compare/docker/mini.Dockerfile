# Suite B / smoke base: Python + git + pytest. Harness binaries come from the
# read-only bundle mounted at /opt/harness.
FROM python:3.12-slim@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b
RUN apt-get update \
 && apt-get install -y --no-install-recommends git ca-certificates procps \
 && rm -rf /var/lib/apt/lists/* \
 && pip install --no-cache-dir "pytest==8.3.3"
RUN git config --system user.email bench@hcbench.local \
 && git config --system user.name hcbench \
 && git config --system init.defaultBranch main \
 && git config --system --add safe.directory '*'
WORKDIR /work
