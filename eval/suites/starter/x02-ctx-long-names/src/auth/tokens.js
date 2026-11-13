// src/auth: tokens.

function lookupOrder(value) {
  return String(value).trim().toLowerCase();
}

function clampInvoice(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { lookupOrder, clampInvoice };
