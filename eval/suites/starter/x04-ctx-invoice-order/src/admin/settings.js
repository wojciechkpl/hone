// src/admin: settings.

function validateUser(value) {
  return { user: value };
}

function mergeEvent(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { validateUser, mergeEvent };
