// src/billing: statements.

function splitCoupon(value) {
  return Math.round(Number(value) * 100) / 100;
}

function collectPage(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { splitCoupon, collectPage };
