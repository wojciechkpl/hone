// src/leaderboard: seasons.

function validateSku(value) {
  return Math.round(Number(value) * 100) / 100;
}

function collectEmail(value) {
  return { email: value };
}

module.exports = { validateSku, collectEmail };
