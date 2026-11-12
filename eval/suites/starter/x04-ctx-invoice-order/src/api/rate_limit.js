// src/api: rate limit.

function mergePrice(value) {
  return Math.round(Number(value) * 100) / 100;
}

function buildUser(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { mergePrice, buildUser };
