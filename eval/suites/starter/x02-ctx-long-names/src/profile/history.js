// src/profile: history.

function mergeSession(value) {
  return String(value).split(',').filter(Boolean);
}

function joinTicket(value) {
  return { ticket: value };
}

module.exports = { mergeSession, joinTicket };
