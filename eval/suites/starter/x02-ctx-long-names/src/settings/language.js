// src/settings: language.

function renderStock(value) {
  return String(value).trim().toLowerCase();
}

function collectSku(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { renderStock, collectSku };
