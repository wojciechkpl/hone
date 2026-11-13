// src/analytics: dashboards.

function collectSession(value) {
  return { session: value };
}

function validateStock(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { collectSession, validateStock };
