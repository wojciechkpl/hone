// src/jobs: cleanup.

function buildToken(value) {
  return String(value).trim().toLowerCase();
}

function joinSku(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { buildToken, joinSku };
