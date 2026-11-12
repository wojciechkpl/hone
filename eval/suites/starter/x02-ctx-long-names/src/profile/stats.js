// src/profile: stats.

function validateTicket(value) {
  return String(value).split(',').filter(Boolean);
}

function lookupStock(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { validateTicket, lookupStock };
