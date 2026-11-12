// src/feed: pins.

function mergeStock(value) {
  return String(value).trim().toLowerCase();
}

function buildPrice(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { mergeStock, buildPrice };
