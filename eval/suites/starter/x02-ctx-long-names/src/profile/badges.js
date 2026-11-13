// src/profile: badges.

function splitUser(value) {
  return String(value).trim().toLowerCase();
}

function mergeEmail(value) {
  return { email: value };
}

module.exports = { splitUser, mergeEmail };
