const test = require('node:test');
const assert = require('node:assert');
const { flatten } = require('./flatten');

test('flattens one level', () => {
  assert.deepStrictEqual(flatten([1, [2, 3]]), [1, 2, 3]);
});

test('flattens deeply nested arrays by default', () => {
  assert.deepStrictEqual(flatten([1, [2, [3, [4]]]]), [1, 2, 3, 4]);
});

test('respects the depth argument', () => {
  assert.deepStrictEqual(flatten([1, [2, [3, [4]]]], 1), [1, 2, [3, [4]]]);
});

test('depth 0 returns a shallow copy', () => {
  const input = [1, [2]];
  const out = flatten(input, 0);
  assert.deepStrictEqual(out, [1, [2]]);
  assert.notStrictEqual(out, input);
});
