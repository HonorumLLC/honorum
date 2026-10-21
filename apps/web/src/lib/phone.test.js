import { test } from 'node:test';
import assert from 'node:assert/strict';
import { formatPhoneInput, normalizePhone } from './phone.js';

test('national and international input map to the same account phone', () => {
  assert.equal(normalizePhone('(201) 555-0123', 'US'), '+12015550123');
  assert.equal(normalizePhone('+1 201 555 0123', 'GB'), '+12015550123');
  assert.equal(normalizePhone('07700 900123', 'GB'), '+447700900123');
  assert.equal(normalizePhone('06 6982', 'IT'), '+39066982');
  for (const input of ['201', 'Call +12015550123', '+12015550123 ext 45', '']) assert.equal(normalizePhone(input), '');
});

test('phone entry formats national numbers and recognizes international pastes', () => {
  assert.equal(formatPhoneInput('2015550123').value, '(201) 555-0123');
  assert.equal(formatPhoneInput('12015550123').value, '(201) 555-0123');
  assert.equal(formatPhoneInput('07700900123', 'GB').value, '07700 900123');
  for (const [value, country, formatted] of [
    ['+1 201 555 0123', 'US', '(201) 555-0123'],
    ['+44 7700 900123', 'GB', '07700 900123'],
    ['+1 416 555 0123', 'CA', '(416) 555-0123'],
  ]) {
    const result = formatPhoneInput(value, 'DE');
    assert.equal(result.country, country);
    assert.equal(result.value, formatted);
    assert.equal(normalizePhone(result.value, result.country), normalizePhone(value));
  }
  assert.equal(formatPhoneInput('(201) 555-0123 ext 45').value, '(201) 555-0123 ext 45');
});

test('phone formatting preserves editing position and lets punctuation be deleted', () => {
  assert.deepEqual(formatPhoneInput('(201', 'US', 4, '(201)', 'deleteContentBackward'), { value: '20', country: 'US', caret: 2 });
  const dash = formatPhoneInput('(201) 5550123', 'US', 9, '(201) 555-0123', 'deleteContentBackward');
  assert.equal(dash.value.replace(/\D/g, ''), '201550123');
  assert.equal(dash.value.slice(0, dash.caret).replace(/\D/g, ''), '20155');
  const middle = formatPhoneInput('(2091) 555-0123', 'US', 4, '(201) 555-0123', 'insertText');
  assert.equal(middle.value.slice(0, middle.caret).replace(/\D/g, ''), '209');
  assert.equal(formatPhoneInput('').value, '');
});
