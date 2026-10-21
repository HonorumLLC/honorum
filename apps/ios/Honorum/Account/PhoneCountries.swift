import Foundation

/// Dialing codes for phone sign-in. The list is factual reference data: the
/// ISO region, its country calling code, and the longest national number that
/// region issues, which is all the entry field needs to know.
enum PhoneCountries {
    struct Country: Identifiable, Hashable, Sendable {
        var id: String
        var code: String
        var longestNationalNumber: Int

        /// The region's name in the reader's language.
        var name: String { Locale.current.localizedString(forRegionCode: id) ?? id }
        var flag: String {
            id.unicodeScalars.compactMap { Unicode.Scalar(127397 + $0.value) }
                .map(String.init).joined()
        }
        var dialing: String { "+" + code }
    }

    /// Every region, sorted by name for the picker.
    static let all: [Country] = raw
        .map { Country(id: $0.0, code: $0.1, longestNationalNumber: $0.2) }
        .sorted { $0.name < $1.name }

    /// The region this device is set to, falling back to the United States.
    static var current: Country {
        let region = Locale.current.region?.identifier ?? "US"
        return all.first { $0.id == region } ?? all.first { $0.id == "US" } ?? all[0]
    }

    static func country(_ id: String) -> Country? { all.first { $0.id == id } }

    /// Accept contact AutoFill and pasted international numbers as well as local input.
    /// Keep the selected region when multiple countries share a calling code.
    static func input(_ text: String, country: Country) -> (country: Country, national: String) {
        var digits = text.filter { $0.isASCII && $0.isNumber }
        var selected = country
        if text.trimmingCharacters(in: .whitespaces).hasPrefix("+") {
            if !digits.hasPrefix(country.code),
                let match = all.sorted(by: { $0.code.count > $1.code.count })
                    .first(where: { digits.hasPrefix($0.code) }) {
                selected = match
            }
            if digits.hasPrefix(selected.code) { digits.removeFirst(selected.code.count) }
        } else if country.code == "1", digits.count == 11, digits.hasPrefix("1") {
            digits.removeFirst()
        }
        return (selected, digits)
    }

    static func formatted(_ national: String, country: Country) -> String {
        let digits = national.filter { $0.isASCII && $0.isNumber }
        guard country.code == "1", digits.count <= 10 else { return digits }
        // Insert separators only before a following digit, so backspace remains natural.
        return digits.enumerated().map { index, digit in
            (index == 3 || index == 6 ? "-" : "") + String(digit)
        }.joined()
    }

    /// What the entry field should show after a change, and for which
    /// country. Typed digits take their separators as they arrive. Text that
    /// starts with a calling code, pasted or from contact AutoFill, is left
    /// alone until it is a complete number, then switches the country and
    /// drops the code.
    static func shaped(_ text: String, country: Country) -> (text: String, country: Country) {
        if text.trimmingCharacters(in: .whitespaces).hasPrefix("+"), !isPlausible(country, text) {
            return (text, country)
        }
        let value = input(text, country: country)
        return (formatted(value.national, country: value.country), value.country)
    }
    static func e164(_ country: Country, _ national: String) -> String {
        let value = input(national, country: country)
        return "+" + value.country.code + value.national
    }

    static func isPlausible(_ country: Country, _ national: String) -> Bool {
        let value = input(national, country: country)
        let count = value.national.count
        if value.country.code == "1" { return count == 10 }
        return count >= 4 && count <= max(value.country.longestNationalNumber, 7)
            && count + value.country.code.count <= 15
    }

    private static let raw: [(String, String, Int)] = [
        ("US", "1", 10),
        ("AG", "1", 10),
        ("AI", "1", 10),
        ("AS", "1", 10),
        ("BB", "1", 10),
        ("BM", "1", 10),
        ("BS", "1", 10),
        ("CA", "1", 10),
        ("DM", "1", 10),
        ("DO", "1", 10),
        ("GD", "1", 10),
        ("GU", "1", 10),
        ("JM", "1", 10),
        ("KN", "1", 10),
        ("KY", "1", 10),
        ("LC", "1", 10),
        ("MP", "1", 10),
        ("MS", "1", 10),
        ("PR", "1", 10),
        ("SX", "1", 10),
        ("TC", "1", 10),
        ("TT", "1", 10),
        ("VC", "1", 10),
        ("VG", "1", 10),
        ("VI", "1", 10),
        ("RU", "7", 14),
        ("KZ", "7", 14),
        ("EG", "20", 10),
        ("ZA", "27", 10),
        ("GR", "30", 12),
        ("NL", "31", 11),
        ("BE", "32", 9),
        ("FR", "33", 9),
        ("ES", "34", 9),
        ("HU", "36", 9),
        ("IT", "39", 12),
        ("VA", "39", 12),
        ("RO", "40", 9),
        ("CH", "41", 12),
        ("AT", "43", 13),
        ("GB", "44", 10),
        ("GG", "44", 10),
        ("IM", "44", 10),
        ("JE", "44", 10),
        ("DK", "45", 8),
        ("SE", "46", 12),
        ("NO", "47", 8),
        ("SJ", "47", 8),
        ("PL", "48", 10),
        ("DE", "49", 15),
        ("PE", "51", 9),
        ("MX", "52", 10),
        ("CU", "53", 10),
        ("AR", "54", 11),
        ("BR", "55", 11),
        ("CL", "56", 11),
        ("CO", "57", 11),
        ("VE", "58", 10),
        ("MY", "60", 10),
        ("AU", "61", 12),
        ("CC", "61", 12),
        ("CX", "61", 12),
        ("ID", "62", 17),
        ("PH", "63", 13),
        ("NZ", "64", 10),
        ("SG", "65", 11),
        ("TH", "66", 13),
        ("JP", "81", 17),
        ("KR", "82", 14),
        ("VN", "84", 10),
        ("CN", "86", 12),
        ("TR", "90", 13),
        ("IN", "91", 13),
        ("PK", "92", 12),
        ("AF", "93", 9),
        ("LK", "94", 9),
        ("MM", "95", 10),
        ("IR", "98", 10),
        ("SS", "211", 9),
        ("MA", "212", 9),
        ("EH", "212", 9),
        ("DZ", "213", 9),
        ("TN", "216", 8),
        ("LY", "218", 9),
        ("GM", "220", 9),
        ("SN", "221", 9),
        ("MR", "222", 8),
        ("ML", "223", 8),
        ("GN", "224", 9),
        ("CI", "225", 10),
        ("BF", "226", 8),
        ("NE", "227", 8),
        ("TG", "228", 8),
        ("BJ", "229", 10),
        ("MU", "230", 10),
        ("LR", "231", 9),
        ("SL", "232", 8),
        ("GH", "233", 9),
        ("NG", "234", 14),
        ("TD", "235", 8),
        ("CF", "236", 8),
        ("CM", "237", 9),
        ("CV", "238", 7),
        ("ST", "239", 7),
        ("GQ", "240", 9),
        ("GA", "241", 8),
        ("CG", "242", 9),
        ("CD", "243", 10),
        ("AO", "244", 9),
        ("GW", "245", 9),
        ("IO", "246", 7),
        ("AC", "247", 6),
        ("SC", "248", 7),
        ("SD", "249", 9),
        ("RW", "250", 9),
        ("ET", "251", 9),
        ("SO", "252", 9),
        ("DJ", "253", 8),
        ("KE", "254", 10),
        ("TZ", "255", 9),
        ("UG", "256", 9),
        ("BI", "257", 8),
        ("MZ", "258", 9),
        ("ZM", "260", 9),
        ("MG", "261", 9),
        ("RE", "262", 9),
        ("YT", "262", 9),
        ("ZW", "263", 10),
        ("NA", "264", 9),
        ("MW", "265", 9),
        ("LS", "266", 8),
        ("BW", "267", 10),
        ("SZ", "268", 9),
        ("KM", "269", 7),
        ("SH", "290", 5),
        ("TA", "290", 4),
        ("ER", "291", 7),
        ("AW", "297", 7),
        ("FO", "298", 6),
        ("GL", "299", 6),
        ("GI", "350", 8),
        ("PT", "351", 9),
        ("LU", "352", 11),
        ("IE", "353", 10),
        ("IS", "354", 9),
        ("AL", "355", 9),
        ("MT", "356", 8),
        ("CY", "357", 8),
        ("FI", "358", 12),
        ("AX", "358", 12),
        ("BG", "359", 12),
        ("LT", "370", 8),
        ("LV", "371", 8),
        ("EE", "372", 10),
        ("MD", "373", 8),
        ("AM", "374", 8),
        ("BY", "375", 11),
        ("AD", "376", 9),
        ("MC", "377", 9),
        ("SM", "378", 10),
        ("UA", "380", 10),
        ("RS", "381", 12),
        ("ME", "382", 9),
        ("XK", "383", 12),
        ("HR", "385", 9),
        ("SI", "386", 8),
        ("BA", "387", 9),
        ("MK", "389", 8),
        ("CZ", "420", 12),
        ("SK", "421", 9),
        ("LI", "423", 9),
        ("FK", "500", 5),
        ("BZ", "501", 11),
        ("GT", "502", 11),
        ("SV", "503", 11),
        ("HN", "504", 11),
        ("NI", "505", 8),
        ("CR", "506", 10),
        ("PA", "507", 11),
        ("PM", "508", 9),
        ("HT", "509", 8),
        ("GP", "590", 9),
        ("BL", "590", 9),
        ("MF", "590", 9),
        ("BO", "591", 9),
        ("GY", "592", 7),
        ("EC", "593", 11),
        ("GF", "594", 9),
        ("PY", "595", 11),
        ("MQ", "596", 9),
        ("SR", "597", 7),
        ("UY", "598", 13),
        ("CW", "599", 8),
        ("BQ", "599", 7),
        ("TL", "670", 8),
        ("NF", "672", 6),
        ("BN", "673", 7),
        ("NR", "674", 7),
        ("PG", "675", 8),
        ("TO", "676", 7),
        ("SB", "677", 7),
        ("VU", "678", 7),
        ("FJ", "679", 11),
        ("PW", "680", 7),
        ("WF", "681", 9),
        ("CK", "682", 5),
        ("NU", "683", 7),
        ("WS", "685", 10),
        ("KI", "686", 8),
        ("NC", "687", 6),
        ("TV", "688", 7),
        ("PF", "689", 9),
        ("TK", "690", 7),
        ("FM", "691", 7),
        ("MH", "692", 7),
        ("KP", "850", 10),
        ("HK", "852", 11),
        ("MO", "853", 8),
        ("KH", "855", 10),
        ("LA", "856", 10),
        ("BD", "880", 10),
        ("TW", "886", 11),
        ("MV", "960", 10),
        ("LB", "961", 8),
        ("JO", "962", 9),
        ("SY", "963", 9),
        ("IQ", "964", 10),
        ("KW", "965", 8),
        ("SA", "966", 10),
        ("YE", "967", 9),
        ("OM", "968", 9),
        ("PS", "970", 10),
        ("AE", "971", 12),
        ("IL", "972", 12),
        ("BH", "973", 8),
        ("QA", "974", 11),
        ("BT", "975", 8),
        ("MN", "976", 10),
        ("NP", "977", 11),
        ("TJ", "992", 9),
        ("TM", "993", 8),
        ("AZ", "994", 9),
        ("GE", "995", 9),
        ("KG", "996", 10),
        ("UZ", "998", 9),
    ]
}
