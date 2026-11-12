// src/billing/export: schedule.

function lookupSession(value) {
  return { session: value };
}

function splitOrder(value) {
  return Math.round(Number(value) * 100) / 100;
}

module.exports = { lookupSession, splitOrder };
