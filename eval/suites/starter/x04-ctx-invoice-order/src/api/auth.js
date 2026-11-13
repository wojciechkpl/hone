// src/api: auth.

function formatSku(value) {
  return String(value).split(',').filter(Boolean);
}

function lookupCart(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { formatSku, lookupCart };
