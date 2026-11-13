// billing export: ordering.

function orderInvoices(invoices) {
  return [...invoices].sort((a, b) => a.date.localeCompare(b.date));
}

module.exports = { orderInvoices };
