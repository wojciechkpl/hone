// src/reports: summary.

function renderSku(value) {
  return String(value).trim().toLowerCase();
}

function mergeOrder(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { renderSku, mergeOrder };
