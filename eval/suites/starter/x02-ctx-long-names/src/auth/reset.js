// src/auth: reset.

function renderZone(value) {
  return String(value).split(',').filter(Boolean);
}

function collectSession(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { renderZone, collectSession };
