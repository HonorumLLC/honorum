import Foundation
import Testing

@testable import Honorum

@Suite("Campus civil dates")
struct CivilDateTests {
    @Test("Parses and reprints an ISO date")
    func roundTrip() {
        let date = CivilDate("2026-09-10")
        #expect(date != nil)
        #expect(date?.iso == "2026-09-10")
        #expect(date?.year == 2026)
        #expect(date?.month == 9)
        #expect(date?.dayOfMonth == 10)
    }

    @Test("Rejects text that is not a date")
    func rejectsGarbage() {
        #expect(CivilDate("") == nil)
        #expect(CivilDate("2026-13-01") == nil)
        #expect(CivilDate("later") == nil)
    }

    @Test("Uses the catalog's Monday-first weekday letters")
    func weekdayLetters() {
        #expect(CivilDate("2026-09-07")?.letter == "M")  // Labor Day, a Monday
        #expect(CivilDate("2026-09-10")?.letter == "R")  // Thursday is R
        #expect(CivilDate("2026-09-13")?.letter == "U")  // Sunday is U
        #expect(CivilDate("1970-01-01")?.letter == "R")  // the epoch was a Thursday
    }

    @Test("Adds days across month and year boundaries")
    func arithmetic() {
        #expect(CivilDate("2026-08-31")?.adding(days: 1).iso == "2026-09-01")
        #expect(CivilDate("2026-12-31")?.adding(days: 1).iso == "2027-01-01")
        #expect(CivilDate("2028-02-28")?.adding(days: 1).iso == "2028-02-29")  // leap year
        #expect(CivilDate("2026-01-01")?.adding(days: -1).iso == "2025-12-31")
    }

    @Test("Starts the week on Sunday")
    func weekStart() {
        // Every day of one week resolves to the same Sunday.
        for offset in 0..<7 {
            let date = CivilDate("2026-09-06")!.adding(days: offset)
            #expect(date.startOfWeek().iso == "2026-09-06")
        }
    }

    @Test("Resolves a campus minute to an instant, including across a DST shift")
    func instants() {
        let chicago = TimeZone(identifier: "America/Chicago")!
        let duringDaylightTime = CivilDate("2026-09-10")!.instant(minute: 9 * 60, in: chicago)
        #expect(CampusClock(duringDaylightTime, in: chicago).minute == 9 * 60)
        #expect(CampusClock(duringDaylightTime, in: chicago).date.iso == "2026-09-10")

        // The first Monday after the November change back to standard time.
        let duringStandardTime = CivilDate("2026-11-09")!.instant(minute: 9 * 60, in: chicago)
        #expect(CampusClock(duringStandardTime, in: chicago).minute == 9 * 60)
        #expect(duringStandardTime.timeIntervalSince(duringDaylightTime) != 0)
    }

    @Test("Reads a device instant as the campus civil date, not the phone's")
    func devicesElsewhere() {
        let tokyo = TimeZone(identifier: "Asia/Tokyo")!
        let chicago = TimeZone(identifier: "America/Chicago")!
        // 2026-09-11 02:00 in Tokyo is still 2026-09-10 in Norman.
        let instant = CivilDate("2026-09-11")!.instant(minute: 2 * 60, in: tokyo)
        #expect(CivilDate(instant, in: tokyo).iso == "2026-09-11")
        #expect(CivilDate(instant, in: chicago).iso == "2026-09-10")
    }
}
