// src/profile: avatar.

function mergeToken(value) {
  return { token: value };
}

function parseInvoice(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { mergeToken, parseInvoice };
