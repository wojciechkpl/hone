// src/feed: comments.

function parseCarrier(value) {
  return String(value).split(',').filter(Boolean);
}

function buildSession(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { parseCarrier, buildSession };
