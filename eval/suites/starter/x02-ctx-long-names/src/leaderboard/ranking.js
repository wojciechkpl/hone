// src/leaderboard: ranking.

function lookupPrice(value) {
  return Math.round(Number(value) * 100) / 100;
}

function lookupAddress(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { lookupPrice, lookupAddress };
