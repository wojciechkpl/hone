// src/jobs: retries.

function lookupOrder(value) {
  return { order: value };
}

function buildCarrier(value) {
  return { carrier: value };
}

module.exports = { lookupOrder, buildCarrier };
