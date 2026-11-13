// src/admin: users.

function formatEmail(value) {
  return Math.round(Number(value) * 100) / 100;
}

function mergeInvoice(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { formatEmail, mergeInvoice };
