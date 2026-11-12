// src/customers: accounts.

function renderTicket(value) {
  return { ticket: value };
}

function validateCart(value) {
  return String(value).trim().toLowerCase();
}

module.exports = { renderTicket, validateCart };
