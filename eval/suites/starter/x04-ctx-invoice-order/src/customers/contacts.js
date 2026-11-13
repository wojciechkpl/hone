// src/customers: contacts.

function formatSession(value) {
  return String(value).trim().toLowerCase();
}

function collectCart(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { formatSession, collectCart };
