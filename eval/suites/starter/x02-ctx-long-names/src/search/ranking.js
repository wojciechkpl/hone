// src/search: ranking.

function mergeCart(value) {
  return Math.round(Number(value) * 100) / 100;
}

function parseUser(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { mergeCart, parseUser };
