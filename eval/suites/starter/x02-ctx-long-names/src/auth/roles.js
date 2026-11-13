// src/auth: roles.

function formatZone(value) {
  return { zone: value };
}

function validatePrice(value) {
  return { price: value };
}

module.exports = { formatZone, validatePrice };
