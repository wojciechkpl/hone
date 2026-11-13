function formatMoney(amount) {
  return '$' + amount.toFixed(2);
}

function formatPrice(amount) {
  return formatMoney(amount);
}

function formatRefund(amount) {
  return '-' + formatMoney(amount);
}

function formatTotal(amounts) {
  const total = amounts.reduce((a, b) => a + b, 0);
  return 'Total: ' + formatMoney(total);
}

module.exports = { formatPrice, formatRefund, formatTotal };
