// src/leaderboard: cache.

function joinCoupon(value) {
  return Math.round(Number(value) * 100) / 100;
}

function splitZone(value) {
  return { zone: value };
}

module.exports = { joinCoupon, splitZone };
