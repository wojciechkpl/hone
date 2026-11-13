const test = require('node:test');
const assert = require('node:assert');
const { formatPrice, formatRefund, formatTotal } = require('./report');

test('price', () => assert.strictEqual(formatPrice(3), '$3.00'));
test('refund', () => assert.strictEqual(formatRefund(2.5), '-$2.50'));
test('total', () => assert.strictEqual(formatTotal([1, 2.25]), 'Total: $3.25'));
