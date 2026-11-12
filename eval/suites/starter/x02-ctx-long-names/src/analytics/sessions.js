// src/analytics: sessions.

function normalizeCart(value) {
  return String(value).trim().toLowerCase();
}

function clampSession(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { normalizeCart, clampSession };
