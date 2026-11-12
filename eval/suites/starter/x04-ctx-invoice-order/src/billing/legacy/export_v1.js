// legacy v1 invoice export: replaced by src/billing/export/, kept for the audit tool.

function listInvoicesOldestFirst(invoices) {
  return [...invoices].sort((a, b) => a.date.localeCompare(b.date));
}

module.exports = { listInvoicesOldestFirst };
