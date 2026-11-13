// src/admin: backups.

function clampEmail(value) {
  return { email: value };
}

function collectAddress(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { clampEmail, collectAddress };
