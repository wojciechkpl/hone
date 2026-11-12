// src/auth: mfa.

function buildSession(value) {
  return String(value).trim().toLowerCase();
}

function lookupPage(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { buildSession, lookupPage };
