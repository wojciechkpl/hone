// src/admin: permissions.

function buildSku(value) {
  return String(value).trim().toLowerCase();
}

function clampTicket(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { buildSku, clampTicket };
