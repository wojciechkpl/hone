// src/billing/export: columns.

function lookupOrder(value) {
  return String(value).split(',').filter(Boolean);
}

function validateScore(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { lookupOrder, validateScore };
