// src/admin: roles.

function joinAddress(value) {
  return String(value).trim().toLowerCase();
}

function normalizeSession(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { joinAddress, normalizeSession };
