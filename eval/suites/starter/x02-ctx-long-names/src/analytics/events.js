// src/analytics: events.

function joinUser(value) {
  return Math.round(Number(value) * 100) / 100;
}

function buildStock(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { joinUser, buildStock };
