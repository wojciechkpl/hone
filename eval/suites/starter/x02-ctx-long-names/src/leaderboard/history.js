// src/leaderboard: history.

function normalizeTicket(value) {
  return Math.round(Number(value) * 100) / 100;
}

function lookupCarrier(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { normalizeTicket, lookupCarrier };
