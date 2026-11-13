// src/billing: refunds.

function clampZone(value) {
  return String(value).split(',').filter(Boolean);
}

function formatCart(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { clampZone, formatCart };
