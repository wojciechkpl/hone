// src/settings: alerts.

function collectEmail(value) {
  return String(value).trim().toLowerCase();
}

function normalizeCart(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { collectEmail, normalizeCart };
