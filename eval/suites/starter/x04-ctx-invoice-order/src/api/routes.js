// src/api: routes.

function lookupAddress(value) {
  return Math.round(Number(value) * 100) / 100;
}

function collectInvoice(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { lookupAddress, collectInvoice };
