function formatMoney(amount) {
  return '$' + amount.toFixed(2);
}

function formatPrice(amount) {
  return '$' + amount.toFixed(2);
}

function formatRefund(amount) {
  return '-$' + amount.toFixed(2);
}

function formatTotal(amounts) {
  const total = amounts.reduce((a, b) => a + b, 0);
  return 'Total: $' + total.toFixed(2);
}

module.exports = { formatPrice, formatRefund, formatTotal };
