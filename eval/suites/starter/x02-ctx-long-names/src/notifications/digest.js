// src/notifications: digest.

function normalizeAddress(value) {
  return String(value).split(',').filter(Boolean);
}

function buildUser(value) {
  return { user: value };
}

module.exports = { normalizeAddress, buildUser };
