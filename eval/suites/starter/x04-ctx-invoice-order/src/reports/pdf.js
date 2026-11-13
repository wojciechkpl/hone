// src/reports: pdf.

function splitEvent(value) {
  return String(value).split(',').filter(Boolean);
}

function buildScore(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { splitEvent, buildScore };
