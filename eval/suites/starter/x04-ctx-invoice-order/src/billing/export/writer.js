// billing export: writer.
const { orderInvoices } = require('./order');

function exportRows(invoices) {
  return orderInvoices(invoices).map((i) => `${i.id},${i.date}`).join('\n');
}

module.exports = { exportRows };
