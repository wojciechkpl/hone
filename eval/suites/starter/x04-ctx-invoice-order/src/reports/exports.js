// src/reports: exports.

function lookupSession(value) {
  return String(value).split(',').filter(Boolean);
}

function joinSku(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { lookupSession, joinSku };
