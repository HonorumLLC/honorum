import SwiftUI
import Testing
@testable import Honorum

@Suite("Opening task sources")
@MainActor
struct TaskSourceOpeningTests {
    private var canvas: StudentTask {
        var task = StudentTask()
        task.source = .canvas
        task.url = "https://canvas.example.edu/courses/12/pages/week%202?module_item_id=34#details"
        return task
    }

    @Test("Canvas destinations preserve encoded paths, queries, and fragments")
    func destination() {
        #expect(canvas.canvasAppURL?.absoluteString == "canvas-courses://canvas.example.edu/courses/12/pages/week%202?module_item_id=34#details")
    }

    @Test("Installed Canvas opens once; unavailable Canvas falls back to the exact website", arguments: [true, false])
    func canvasRouting(installed: Bool) {
        var opened: [URL] = []
        let action = OpenURLAction { url in
            opened.append(url)
            return url.scheme == "canvas-courses" && !installed ? .discarded : .handled
        }
        canvas.openSource(using: action)
        #expect(opened == (installed ? [canvas.canvasAppURL!] : [canvas.canvasAppURL!, canvas.sourceURL!]))
    }

    @Test("Calendar imports use the website even when it has a Canvas path")
    func feedRouting() {
        var task = canvas
        task.source = .feed
        var opened: [URL] = []
        task.openSource(using: OpenURLAction { opened.append($0); return .handled })
        #expect(task.canvasAppURL == nil)
        #expect(opened == [task.sourceURL!])
    }

    @Test("Invalid and manual sources never open anything")
    func invalidRouting() {
        var opened: [URL] = []
        let action = OpenURLAction { opened.append($0); return .handled }
        var task = canvas
        task.source = .manual
        task.openSource(using: action)
        task.source = .canvas
        for address in ["", "javascript:alert(1)", "canvas-courses://canvas.example.edu", "https://user:secret@canvas.example.edu"] {
            task.url = address
            #expect(task.canvasAppURL == nil)
            task.openSource(using: action)
        }
        #expect(opened.isEmpty)
    }
}
