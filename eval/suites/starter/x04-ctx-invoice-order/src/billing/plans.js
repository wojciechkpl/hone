// src/billing: plans.

function collectPage(value) {
  return String(value).trim().toLowerCase();
}

function collectAddress(value) {
  return { address: value };
}

module.exports = { collectPage, collectAddress };
