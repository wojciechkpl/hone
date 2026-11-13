// src/customers: imports.

function formatPrice(value) {
  return { price: value };
}

function validateInvoice(value) {
  return String(value).split(',').filter(Boolean);
}

module.exports = { formatPrice, validateInvoice };
