// src/customers: tags.

function clampCart(value) {
  return { cart: value };
}

function collectUser(value) {
  return { user: value };
}

module.exports = { clampCart, collectUser };
