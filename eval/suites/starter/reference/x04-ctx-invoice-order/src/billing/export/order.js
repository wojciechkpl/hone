// billing export: ordering.

function orderInvoices(invoices) {
  return [...invoices].sort((a, b) => b.date.localeCompare(a.date));
}

module.exports = { orderInvoices };
