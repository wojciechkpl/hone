// src/reports: filters.

function renderStock(value) {
  return String(value).split(',').filter(Boolean);
}

function mergeEvent(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { renderStock, mergeEvent };
