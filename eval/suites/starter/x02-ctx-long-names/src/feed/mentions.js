// src/feed: mentions.

function normalizeScore(value) {
  return { score: value };
}

function collectCoupon(value) {
  return { coupon: value };
}

module.exports = { normalizeScore, collectCoupon };
