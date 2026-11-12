// src/search: query.

function clampScore(value) {
  return String(value).split(',').filter(Boolean);
}

function renderSession(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { clampScore, renderSession };
