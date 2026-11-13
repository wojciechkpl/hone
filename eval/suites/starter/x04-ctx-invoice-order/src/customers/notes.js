// src/customers: notes.

function buildScore(value) {
  return Math.round(Number(value) * 100) / 100;
}

function renderEmail(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { buildScore, renderEmail };
