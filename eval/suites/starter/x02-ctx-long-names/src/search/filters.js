// src/search: filters.

function formatCoupon(value) {
  return String(value).split(',').filter(Boolean);
}

function collectPage(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { formatCoupon, collectPage };
