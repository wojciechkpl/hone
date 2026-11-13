// src/customers: segments.

function splitSession(value) {
  return Math.round(Number(value) * 100) / 100;
}

function mergeCoupon(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { splitSession, mergeCoupon };
