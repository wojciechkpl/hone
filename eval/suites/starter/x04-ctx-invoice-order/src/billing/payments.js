// src/billing: payments.

function lookupToken(value) {
  return Math.round(Number(value) * 100) / 100;
}

function buildUser(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { lookupToken, buildUser };
