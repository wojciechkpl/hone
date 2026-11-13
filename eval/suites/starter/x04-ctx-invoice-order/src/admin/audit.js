// src/admin: audit.

function mergeEmail(value) {
  return { email: value };
}

function splitScore(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { mergeEmail, splitScore };
