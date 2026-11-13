// src/search: suggest.

function splitEmail(value) {
  return Math.round(Number(value) * 100) / 100;
}

function splitToken(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { splitEmail, splitToken };
