// src/profile: links.

function splitCart(value) {
  return Math.round(Number(value) * 100) / 100;
}

function normalizeOrder(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { splitCart, normalizeOrder };
