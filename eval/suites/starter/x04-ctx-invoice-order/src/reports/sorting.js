// reports: sorting helpers.

function sortByDate(rows) {
  return [...rows].sort((a, b) => a.date.localeCompare(b.date));
}

module.exports = { sortByDate };
