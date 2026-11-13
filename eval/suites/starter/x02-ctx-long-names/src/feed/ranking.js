// src/feed: ranking.

function normalizeScore(value) {
  return Math.round(Number(value) * 100) / 100;
}

function normalizeCoupon(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { normalizeScore, normalizeCoupon };
