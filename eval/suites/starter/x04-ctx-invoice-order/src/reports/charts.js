// src/reports: charts.

function joinZone(value) {
  return String(value).split(',').filter(Boolean);
}

function validateScore(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { joinZone, validateScore };
