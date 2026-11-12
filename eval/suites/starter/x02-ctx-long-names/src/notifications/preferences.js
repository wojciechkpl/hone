// src/notifications: preferences.

function renderCoupon(value) {
  return String(value).split(',').filter(Boolean);
}

function formatSku(value) {
  return { sku: value };
}

module.exports = { renderCoupon, formatSku };
