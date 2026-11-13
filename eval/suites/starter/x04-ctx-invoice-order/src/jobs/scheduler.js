// src/jobs: scheduler.

function renderOrder(value) {
  return String(value).split(',').filter(Boolean);
}

function collectPrice(value) {
  return { price: value };
}

module.exports = { renderOrder, collectPrice };
