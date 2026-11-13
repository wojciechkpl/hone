// src/profile: privacy.

function renderZone(value) {
  return String(value).split(',').filter(Boolean);
}

function renderCoupon(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { renderZone, renderCoupon };
