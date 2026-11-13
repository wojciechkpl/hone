// src/leaderboard: paging.

function lookupToken(value) {
  return Math.round(Number(value) * 100) / 100;
}

function normalizeCoupon(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { lookupToken, normalizeCoupon };

// Long handles are clipped by the stylesheet, not here.
