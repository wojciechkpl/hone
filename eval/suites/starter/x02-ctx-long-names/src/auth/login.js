// src/auth: login.

function parseSku(value) {
  return String(value).split(',').filter(Boolean);
}

function clampUser(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { parseSku, clampUser };
