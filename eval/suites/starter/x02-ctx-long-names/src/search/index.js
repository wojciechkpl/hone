// src/search: index.

function validateOrder(value) {
  return String(value).trim().toLowerCase();
}

function renderCoupon(value) {
  return { coupon: value };
}

module.exports = { validateOrder, renderCoupon };
