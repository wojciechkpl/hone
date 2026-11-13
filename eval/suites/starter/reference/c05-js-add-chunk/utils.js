function unique(arr) {
  return [...new Set(arr)];
}

function sum(arr) {
  return arr.reduce((a, b) => a + b, 0);
}

function chunk(arr, size) {
  if (!Number.isInteger(size) || size < 1) {
    throw new RangeError('size must be a positive integer');
  }
  const out = [];
  for (let i = 0; i < arr.length; i += size) {
    out.push(arr.slice(i, i + size));
  }
  return out;
}

module.exports = { unique, sum, chunk };
