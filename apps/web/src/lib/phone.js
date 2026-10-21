import { AsYouType, getCountries, getCountryCallingCode, parseIncompletePhoneNumber, parsePhoneNumberFromString } from 'libphonenumber-js/min';

// Loaded with the sign-in dialog, outside the planner's entry bundle.
const names = new Intl.DisplayNames(undefined, { type: 'region' });
export const countries = getCountries().map(code => ({ code, name: names.of(code), callingCode: getCountryCallingCode(code) }))
  .sort((a, b) => a.name.localeCompare(b.name));

export function normalizePhone(value, country = 'US') {
  const phone = parsePhoneNumberFromString(value, { defaultCountry: country, extract: false });
  return phone && !phone.ext && phone.isPossible() ? phone.number : '';
}

// Keep the caret beside the edited digit as punctuation is inserted. Deleting
// a formatting character also removes the adjacent digit, so backspace cannot
// get stuck on a parenthesis or dash that the formatter keeps adding back.
export function formatPhoneInput(value, country = 'US', caret = value.length, previous = '', inputType = '') {
  const digits = text => parseIncompletePhoneNumber(text).replace(/^\+/, '');
  if (inputType.startsWith('delete') && value !== previous && digits(value) === digits(previous)) {
    const backwards = inputType === 'deleteContentBackward';
    const at = backwards ? value.slice(0, caret).search(/\d\D*$/) : value.slice(caret).search(/\d/);
    if (at >= 0) {
      const index = backwards ? at : caret + at;
      value = value.slice(0, index) + value.slice(index + 1);
      if (index < caret) caret--;
    }
  }
  const atEnd = caret === value.length;
  let before = digits(value.slice(0, caret)).length;
  const originalDigits = digits(value).length;
  const parsed = parsePhoneNumberFromString(value, { defaultCountry: country, extract: false });
  // Keep extensions visible and invalid instead of turning them into a
  // different phone number by silently appending their digits.
  if (parsed?.ext) return { value, country, caret };
  if (parsed?.isPossible()) {
    if (value.trim().startsWith('+')) {
      const matching = getCountries().filter(code => getCountryCallingCode(code) === parsed.countryCallingCode);
      country = parsed.country || (matching.includes(country) ? country : matching[0]) || country;
    }
    // Country calling codes live in the selector; preserve the national trunk
    // prefix (for example Britain's 0) when a full international number is pasted.
    if (getCountryCallingCode(country) === parsed.countryCallingCode) value = parsed.formatNational();
  }
  const formatted = new AsYouType(country).input(parseIncompletePhoneNumber(value));
  before = Math.max(0, before - originalDigits + digits(formatted).length);
  let position = 0, seen = 0;
  while (position < formatted.length && seen < before) {
    if (/\d/.test(formatted[position])) seen++;
    position++;
  }
  return { value: formatted, country, caret: atEnd ? formatted.length : position };
}
