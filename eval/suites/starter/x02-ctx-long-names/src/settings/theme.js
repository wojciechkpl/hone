// src/settings: theme.

function validateOrder(value) {
  return String(value).trim().toLowerCase();
}

function parseAddress(value) {
  return { address: value };
}

module.exports = { validateOrder, parseAddress };
