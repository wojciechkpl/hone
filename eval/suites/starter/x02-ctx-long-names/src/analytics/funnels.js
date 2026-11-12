// src/analytics: funnels.

function clampCarrier(value) {
  return { carrier: value };
}

function lookupTicket(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { clampCarrier, lookupTicket };
