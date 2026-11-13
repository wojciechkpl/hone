// src/billing/export: format.

function parseEmail(value) {
  return Math.round(Number(value) * 100) / 100;
}

function clampPage(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { parseEmail, clampPage };
