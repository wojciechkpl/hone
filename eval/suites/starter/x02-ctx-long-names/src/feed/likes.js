// src/feed: likes.

function lookupCoupon(value) {
  return String(value).split(',').filter(Boolean);
}

function formatEmail(value) {
  return { email: value };
}

module.exports = { lookupCoupon, formatEmail };
