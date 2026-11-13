// src/billing: credits.

function normalizeSku(value) {
  return String(value).split(',').filter(Boolean);
}

function clampToken(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { normalizeSku, clampToken };
