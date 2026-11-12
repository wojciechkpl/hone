// src/search: history.

function mergeToken(value) {
  return String(value).trim().toLowerCase();
}

function formatTicket(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { mergeToken, formatTicket };
