import Foundation
import Testing

@testable import Honorum

@Suite("Class search")
struct CatalogIndexTests {
    private func meeting(_ days: String, _ start: Int) -> Meeting {
        Meeting(days: days, start: start, end: start + 50, location: "Dale Hall")
    }

    private func catalog() -> CatalogFile {
        CatalogFile(
            inst: "ou",
            term: Term(id: "202610", name: "Fall 2026", start: "2026-08-24", end: "2026-12-18"),
            courses: [
                Course(
                    code: "C S 2334", subject: "C S", title: "Programming Structures and Abstractions",
                    sections: [
                        CourseSection(
                            id: "10001", num: "001", type: "LEC", instructors: ["Dean Hougen"],
                            meetings: [meeting("MWF", 540)], credits: 3, mode: "In person"),
                        CourseSection(
                            id: "10002", num: "002", type: "LEC",
                            meetings: [meeting("TR", 1080)], credits: 3, mode: "In person"),
                    ]),
                Course(
                    code: "MIS 3033", subject: "MIS", title: "Business Analytics",
                    sections: [
                        CourseSection(
                            id: "20001", num: "010", type: "LEC", instructors: ["Ann Davidson"],
                            meetings: [meeting("TR", 630)], credits: 3, mode: "Online")
                    ]),
                Course(
                    code: "CHEM 1315", subject: "CHEM", title: "General Chemistry",
                    sections: [
                        CourseSection(
                            id: "30001", num: "001", type: "LEC",
                            meetings: [meeting("MWF", 660)], credits: 5, mode: "In person")
                    ]),
            ])
    }

    @Test("Exam refreshes are limited to the catalog's school and term")
    func publishedExams() async {
        let index = CatalogIndex()
        var file = catalog()
        let exam = Exam(date: "2026-12-14", start: 540, end: 660)
        file.courses[0].sections[0].exams = [exam]
        await index.adopt(file)
        let exams = await index.publishedExams(inst: "ou", term: "202610")
        #expect(exams["10001"] == [exam])
        #expect(exams["10002"] == [])
        #expect(await index.publishedExams(inst: "another", term: "202610").isEmpty)
        #expect(await index.publishedExams(inst: "ou", term: "another").isEmpty)
    }

    private func index() async -> CatalogIndex {
        let index = CatalogIndex()
        await index.adopt(catalog())
        return index
    }

    @Test("An empty query browses the whole term in code order")
    func browse() async {
        let found = await index().search("", filters: CatalogFilters(), limit: 40)
        #expect(found.total == 3)
        #expect(found.courses.map(\.code) == ["C S 2334", "CHEM 1315", "MIS 3033"])
    }

    @Test("A subject beats a word buried in another course's title")
    func subjectOutranksSubstring() async {
        let found = await index().search("mis", filters: CatalogFilters(), limit: 40)
        #expect(found.courses.first?.code == "MIS 3033")
    }

    @Test("Separators between a code and a section number are ignored")
    func separators() async {
        for query in ["cs2334", "c s 2334", "C-S-2334"] {
            let found = await index().search(query, filters: CatalogFilters(), limit: 40)
            #expect(found.courses.first?.code == "C S 2334", "\(query) should find C S 2334")
        }
    }

    @Test("A course plus a section number narrows to that section")
    func narrowsToSection() async {
        let found = await index().search("c s 2334 2", filters: CatalogFilters(), limit: 40)
        #expect(found.courses.count == 1)
        #expect(found.courses.first?.sections.map(\.num) == ["002"])
    }

    @Test("A bare section id finds its own section")
    func findsBySectionID() async {
        let found = await index().search("20001", filters: CatalogFilters(), limit: 40)
        #expect(found.courses.first?.code == "MIS 3033")
        #expect(found.courses.first?.sections.count == 1)
    }

    @Test("Every token has to match somewhere")
    func allTokensMatch() async {
        let found = await index().search("chemistry hougen", filters: CatalogFilters(), limit: 40)
        #expect(found.total == 0)
    }

    @Test("Filters trim a course to its passing sections and drop the rest")
    func filters() async {
        var morning = CatalogFilters()
        morning.time = .morning
        let found = await index().search("c s 2334", filters: morning, limit: 40)
        #expect(found.courses.first?.sections.map(\.num) == ["001"])

        var online = CatalogFilters()
        online.mode = "Online"
        let onlineOnly = await index().search("", filters: online, limit: 40)
        #expect(onlineOnly.courses.map(\.code) == ["MIS 3033"])

        var upperLevel = CatalogFilters()
        upperLevel.level = "3"
        let byLevel = await index().search("", filters: upperLevel, limit: 40)
        #expect(byLevel.courses.map(\.code) == ["MIS 3033"])
    }

    @Test("The result reports the whole match count, not just the page")
    func paging() async {
        let page = await index().search("", filters: CatalogFilters(), limit: 2)
        #expect(page.courses.count == 2)
        #expect(page.total == 3)
    }

    @Test("Open-seat filtering uses the counts the term published")
    func openSeats() async {
        let catalog = CatalogIndex()
        await catalog.adopt(self.catalog())
        await catalog.merge(live: [
            "10001": Seats(capacity: 100, available: 0, waitlist: 3),
            "10002": Seats(capacity: 100, available: 7, waitlist: 0),
        ])
        var open = CatalogFilters()
        open.openOnly = true
        let found = await catalog.search("c s", filters: open, limit: 40)
        #expect(found.courses.first?.sections.map(\.num) == ["002"])
    }
}
