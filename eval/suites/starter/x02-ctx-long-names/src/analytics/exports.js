// src/analytics: exports.

function clampCarrier(value) {
  return { carrier: value };
}

function parseSku(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { clampCarrier, parseSku };
