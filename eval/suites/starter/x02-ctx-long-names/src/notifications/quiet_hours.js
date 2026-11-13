// src/notifications: quiet hours.

function splitPrice(value) {
  return Math.round(Number(value) * 100) / 100;
}

function normalizeCarrier(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { splitPrice, normalizeCarrier };
