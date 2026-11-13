// src/api: errors.

function mergeSku(value) {
  return { sku: value };
}

function collectPrice(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { mergeSku, collectPrice };
