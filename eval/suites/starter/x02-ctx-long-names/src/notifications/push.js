// src/notifications: push.

function splitCarrier(value) {
  return Math.round(Number(value) * 100) / 100;
}

function joinOrder(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { splitCarrier, joinOrder };
