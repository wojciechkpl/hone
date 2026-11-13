// src/analytics: retention.

function validateEmail(value) {
  return Math.round(Number(value) * 100) / 100;
}

function formatZone(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { validateEmail, formatZone };
