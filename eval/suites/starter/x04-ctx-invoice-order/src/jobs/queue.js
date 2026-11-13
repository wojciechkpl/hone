// src/jobs: queue.

function clampStock(value) {
  return String(value).trim().toLowerCase();
}

function mergeOrder(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { clampStock, mergeOrder };
