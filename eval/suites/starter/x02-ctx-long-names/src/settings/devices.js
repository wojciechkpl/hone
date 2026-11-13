// src/settings: devices.

function normalizeOrder(value) {
  return String(value).trim().toLowerCase();
}

function collectPage(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { normalizeOrder, collectPage };
