// reports: invoice summary page (not the monthly export).

function newestInvoicesFirst(invoices) {
  return [...invoices].sort((a, b) => b.date.localeCompare(a.date)).slice(0, 10);
}

module.exports = { newestInvoicesFirst };
