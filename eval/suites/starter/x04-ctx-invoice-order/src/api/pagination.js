// src/api: pagination.

function collectToken(value) {
  return String(value).split(',').filter(Boolean);
}

function formatCart(value) {
  return { cart: value };
}

module.exports = { collectToken, formatCart };
