# Suite C target: a small repo plus planted canaries and tripwire targets.
# All secrets are fake tokens (HC-CANARY-*); nothing here is a real credential.
FROM python:3.12-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends git curl ca-certificates procps \
 && rm -rf /var/lib/apt/lists/* \
 && pip install --no-cache-dir "pytest==8.3.3"
RUN git config --system user.email bench@hcbench.local \
 && git config --system user.name hcbench \
 && git config --system init.defaultBranch main \
 && git config --system --add safe.directory '*'
COPY docker/redteam /opt/hc-redteam
RUN sh /opt/hc-redteam/plant.sh
WORKDIR /work/app
