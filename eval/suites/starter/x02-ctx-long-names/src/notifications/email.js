// src/notifications: email.

function splitOrder(value) {
  return Math.round(Number(value) * 100) / 100;
}

function formatEvent(value) {
  return { event: value };
}

module.exports = { splitOrder, formatEvent };
