// src/auth: sessions.

function lookupTicket(value) {
  return String(value).trim().toLowerCase();
}

function mergeToken(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { lookupTicket, mergeToken };
