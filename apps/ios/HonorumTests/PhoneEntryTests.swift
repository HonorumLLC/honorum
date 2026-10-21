import Testing
@testable import Honorum

@Suite("Phone AutoFill")
struct PhoneEntryTests {
    private let us = PhoneCountries.country("US")!

    @Test("Local, pasted and contact AutoFill values submit exactly one calling code",
          arguments: ["4155550123", "415-555-0123", "+1 (415) 555-0123", "1 415 555 0123"])
    func northAmerica(_ text: String) {
        #expect(PhoneCountries.e164(us, text) == "+14155550123")
        #expect(PhoneCountries.isPlausible(us, text))
        let value = PhoneCountries.input(text, country: us)
        #expect(PhoneCountries.formatted(value.national, country: value.country) == "415-555-0123")
    }

    @Test("International AutoFill updates the country instead of adding +1")
    func international() {
        let value = PhoneCountries.input("+44 7700 900123", country: us)
        #expect(value.country.code == "44")
        #expect(value.national == "7700900123")
        #expect(PhoneCountries.e164(value.country, value.national) == "+447700900123")
        let canada = PhoneCountries.country("CA")!
        #expect(PhoneCountries.input("+1 416 555 0123", country: canada).country.id == "CA")
    }

    @Test("Digits are shaped as they arrive; a calling code waits until the number is whole")
    func shaping() {
        #expect(PhoneCountries.shaped("4155", country: us).text == "415-5")
        #expect(PhoneCountries.shaped("415555", country: us).text == "415-555")
        #expect(PhoneCountries.shaped("415-5550", country: us).text == "415-555-0")
        #expect(PhoneCountries.shaped("+1415", country: us).text == "+1415")
        let done = PhoneCountries.shaped("+14155550123", country: us)
        #expect(done.text == "415-555-0123" && done.country.id == "US")
        let abroad = PhoneCountries.shaped("+44 7700 900123", country: us)
        #expect(abroad.text == "7700900123" && abroad.country.code == "44")
    }

    @Test("Partial formatting remains editable and incomplete US numbers cannot submit")
    func editing() {
        #expect(PhoneCountries.formatted("415", country: us) == "415")
        #expect(PhoneCountries.formatted("4155", country: us) == "415-5")
        #expect(PhoneCountries.formatted("415555", country: us) == "415-555")
        #expect(!PhoneCountries.isPlausible(us, "415555"))
        #expect(!PhoneCountries.isPlausible(us, "415555012345"))
    }
}
