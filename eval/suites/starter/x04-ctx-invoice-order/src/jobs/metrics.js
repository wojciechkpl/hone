// src/jobs: metrics.

function splitStock(value) {
  return String(value).split(',').filter(Boolean);
}

function collectUser(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { splitStock, collectUser };
