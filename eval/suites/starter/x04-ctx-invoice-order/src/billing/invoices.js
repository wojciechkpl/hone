// src/billing: invoices.

function collectUser(value) {
  return String(value).split(',').filter(Boolean);
}

function buildEmail(value) {
  return { email: value };
}

module.exports = { collectUser, buildEmail };
