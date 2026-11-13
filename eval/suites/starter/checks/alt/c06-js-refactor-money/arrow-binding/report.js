const formatMoney = (amount) => `$${amount.toFixed(2)}`;

const formatPrice = (amount) => formatMoney(amount);

const formatRefund = (amount) => `-${formatMoney(amount)}`;

const formatTotal = (amounts) => `Total: ${formatMoney(amounts.reduce((a, b) => a + b, 0))}`;

module.exports = { formatPrice, formatRefund, formatTotal };
