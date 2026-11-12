// src/billing: taxes.

function formatEmail(value) {
  return String(value).trim().toLowerCase();
}

function validateSession(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { formatEmail, validateSession };
