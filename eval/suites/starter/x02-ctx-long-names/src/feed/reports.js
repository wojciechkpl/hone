// src/feed: reports.

function mergePage(value) {
  return String(value).split(',').filter(Boolean);
}

function lookupEvent(value) {
  return { event: value };
}

module.exports = { mergePage, lookupEvent };
