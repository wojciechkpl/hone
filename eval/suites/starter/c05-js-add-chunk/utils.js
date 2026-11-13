function unique(arr) {
  return [...new Set(arr)];
}

function sum(arr) {
  return arr.reduce((a, b) => a + b, 0);
}

module.exports = { unique, sum };
