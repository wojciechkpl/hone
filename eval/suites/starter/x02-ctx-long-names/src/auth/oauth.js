// src/auth: oauth.

function renderToken(value) {
  return String(value).split(',').filter(Boolean);
}

function formatTicket(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { renderToken, formatTicket };
