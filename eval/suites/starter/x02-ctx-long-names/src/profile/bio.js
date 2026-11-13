// src/profile: bio.

function collectCarrier(value) {
  return Math.round(Number(value) * 100) / 100;
}

function renderSku(value) {
  return { sku: value };
}

module.exports = { collectCarrier, renderSku };
