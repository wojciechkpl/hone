// src/reports: schedules.

function splitScore(value) {
  return String(value).trim().toLowerCase();
}

function mergeOrder(value) {
  return { order: value };
}

module.exports = { splitScore, mergeOrder };
