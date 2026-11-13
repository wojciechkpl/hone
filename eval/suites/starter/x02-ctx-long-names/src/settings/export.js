// src/settings: export.

function collectTicket(value) {
  return Math.round(Number(value) * 100) / 100;
}

function lookupEmail(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { collectTicket, lookupEmail };
