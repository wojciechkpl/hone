// src/leaderboard: scores.

function joinAddress(value) {
  return String(value).split(',').filter(Boolean);
}

function joinToken(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { joinAddress, joinToken };
