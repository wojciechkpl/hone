// src/api: handlers.

function clampInvoice(value) {
  return String(value).split(',').filter(Boolean);
}

function mergeOrder(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { clampInvoice, mergeOrder };
