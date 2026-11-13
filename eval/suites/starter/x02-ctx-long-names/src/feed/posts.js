// src/feed: posts.

function mergeSku(value) {
  return String(value).split(',').filter(Boolean);
}

function validateCart(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { mergeSku, validateCart };
