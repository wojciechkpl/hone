// src/api: validation.

function formatSku(value) {
  return { sku: value };
}

function lookupCarrier(value) {
  return { carrier: value };
}

module.exports = { formatSku, lookupCarrier };
