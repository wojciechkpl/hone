const test = require('node:test');
const assert = require('node:assert');
const { unique, sum, chunk } = require('./utils');

test('unique', () => {
  assert.deepStrictEqual(unique([1, 1, 2]), [1, 2]);
});

test('sum', () => {
  assert.strictEqual(sum([1, 2, 3]), 6);
});

test('chunk splits into groups', () => {
  assert.deepStrictEqual(chunk([1, 2, 3, 4, 5], 2), [[1, 2], [3, 4], [5]]);
});

test('chunk of an empty array is empty', () => {
  assert.deepStrictEqual(chunk([], 3), []);
});

test('chunk rejects a bad size', () => {
  assert.throws(() => chunk([1], 0), RangeError);
});
