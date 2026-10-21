import XCTest

@MainActor
final class NativeFlowTests: XCTestCase {
    private func launch(_ tab: String = "schedule", welcome: Bool = false, extra: [String] = []) -> XCUIApplication {
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launchArguments = ["-uiTesting", "-startTab", tab] + (welcome ? ["-welcome"] : []) + extra
        app.launch()
        return app
    }

    /// Pops back to Home, whatever is pushed over it, and brings its header
    /// back into view.
    private func returnHome(_ app: XCUIApplication) {
        let home = app.buttons["accountButton"]
        for _ in 0..<4 where !home.isHittable {
            let back = app.navigationBars.buttons.firstMatch
            guard back.exists, back.isHittable else { break }
            back.tap()
            _ = home.waitForExistence(timeout: 2)
        }
        for _ in 0..<4 where !home.isHittable { app.swipeDown() }
        XCTAssertTrue(home.isHittable)
    }

    /// Scrolls the page up until the element can be tapped.
    private func reveal(_ element: XCUIElement, in app: XCUIApplication) {
        for _ in 0..<6 where !element.isHittable { app.swipeUp(velocity: .slow) }
        XCTAssertTrue(element.isHittable)
    }

    /// The one hittable element of several that share an identifier, like
    /// the add button of the screen on top.
    private func onscreen(_ query: XCUIElementQuery) -> XCUIElement {
        query.allElementsBoundByIndex.first { $0.isHittable } ?? query.firstMatch
    }

    private func capture(_ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    func testTickedTaskFoldsIntoCompletedAndSorts() {
        let app = launch("tasks")
        let list = app.scrollViews["tasks-list"]
        XCTAssertTrue(list.buttons["task-fixture-0"].waitForExistence(timeout: 5))
        XCTAssertTrue(list.staticTexts["Tomorrow"].exists)
        list.buttons["tick-fixture-0"].tap()
        // The tick lands at once; the task then folds into the completed row
        // and the rest of the list stays.
        let gone = expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: list.buttons["task-fixture-0"])
        wait(for: [gone], timeout: 2)
        XCTAssertTrue(list.buttons["task-fixture-1"].exists)
        let fold = list.buttons["completedToggle"]
        XCTAssertTrue(fold.waitForExistence(timeout: 3))
        capture("ticked-row-folded")
        fold.tap()
        let done = list.buttons["tick-fixture-0"]
        XCTAssertTrue(done.waitForExistence(timeout: 3))
        XCTAssertEqual(done.label, "Mark Not Done")
        done.tap()
        XCTAssertTrue(list.buttons["task-fixture-0"].waitForExistence(timeout: 3))
        XCTAssertFalse(fold.exists)
        // Sorting by title trades the day headings for one list.
        app.buttons["Sort"].tap()
        app.buttons["Title"].tap()
        XCTAssertTrue(list.buttons["task-fixture-1"].waitForExistence(timeout: 3))
        XCTAssertFalse(list.staticTexts["Tomorrow"].exists)
        capture("tasks-by-title")
    }

    func testTickingOnHomeMovesTheRings() {
        let app = launch("home", extra: ["-demoFixture"])
        let today = app.descendants(matching: .any)["ring-today"].firstMatch
        XCTAssertTrue(today.waitForExistence(timeout: 5))
        XCTAssertTrue(today.label.contains("1 of 4 done"), today.label)
        app.buttons["tick-demo-2"].tap()
        XCTAssertTrue(today.wait(forLabelContaining: "2 of 4 done"), today.label)
        let fold = app.buttons["completedToggle"].firstMatch
        XCTAssertTrue(fold.waitForExistence(timeout: 3))
        XCTAssertTrue(fold.label.contains("2 completed"), fold.label)
        capture("home-ticked")
        fold.tap()
        app.buttons["tick-demo-2"].tap()
        XCTAssertTrue(today.wait(forLabelContaining: "1 of 4 done"), today.label)
    }

    func testHomeOpensEveryScreen() {
        let app = launch("home", extra: ["-demoFixture"])
        let task = app.buttons["task-demo-4"]
        XCTAssertTrue(task.waitForExistence(timeout: 5))
        capture("home")
        task.tap()
        XCTAssertTrue(app.navigationBars["Edit Task"].waitForExistence(timeout: 3))
        app.buttons["Cancel"].tap()
        let calendar = app.buttons["openSchedule"]
        reveal(calendar, in: app)
        capture("home-schedule")
        calendar.tap()
        XCTAssertTrue(app.scrollViews["day-pages"].waitForExistence(timeout: 3))
        returnHome(app)
        let all = app.buttons["allTasks"]
        reveal(all, in: app)
        all.tap()
        XCTAssertTrue(app.scrollViews["tasks-list"].waitForExistence(timeout: 3))
        returnHome(app)
        // Home's date and Account live in the system's navigation bar, which
        // gives pull to refresh a place to show its spinner.
        XCTAssertTrue(app.navigationBars.buttons["accountButton"].exists)
        app.buttons["accountButton"].tap()
        XCTAssertTrue(app.navigationBars["Account"].waitForExistence(timeout: 3))
        returnHome(app)
        // Search finds tasks in place: the bottom bar's search becomes the
        // system's field on the keyboard, with Home and its bar still in
        // view until something is typed, and then the results replace Home.
        app.buttons["Search"].firstMatch.tap()
        let search = app.searchFields.firstMatch
        XCTAssertTrue(search.waitForExistence(timeout: 5))
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3))
        let results = app.collectionViews["search-results"]
        XCTAssertFalse(results.exists)
        XCTAssertTrue(app.navigationBars.buttons["accountButton"].exists)
        search.typeText("Lab")
        XCTAssertTrue(results.buttons["task-demo-4"].waitForExistence(timeout: 3))
        XCTAssertFalse(results.buttons["task-demo-2"].exists)
        capture("search")
        // A result opens over the search, which is still there after.
        results.buttons["task-demo-4"].tap()
        XCTAssertTrue(app.navigationBars["Edit Task"].waitForExistence(timeout: 3))
        app.buttons["Cancel"].tap()
        XCTAssertTrue(results.buttons["task-demo-4"].waitForExistence(timeout: 3))
        // Closing the search brings Home back.
        app.buttons.matching(NSPredicate(format: "label ==[c] 'close'")).firstMatch.tap()
        XCTAssertTrue(app.buttons["task-demo-2"].waitForExistence(timeout: 3))
        XCTAssertFalse(results.exists)
        // New makes a task or an event.
        app.buttons["addButton"].tap()
        XCTAssertTrue(app.navigationBars["New Task"].waitForExistence(timeout: 3))
        app.segmentedControls["newKind"].buttons["Event"].tap()
        XCTAssertTrue(app.navigationBars["New Event"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.descendants(matching: .any)["event-date"].firstMatch.exists)
        app.buttons["Cancel"].tap()
    }

    func testTaskListReachesItsLastRow() {
        let app = launch("tasks", extra: ["-manyTasksFixture"])
        let list = app.scrollViews["tasks-list"]
        XCTAssertTrue(list.waitForExistence(timeout: 5))
        for _ in 0..<5 { list.swipeUp() }
        let last = list.buttons["tick-extra-18"]
        XCTAssertTrue(last.isHittable)
        XCTAssertLessThan(last.frame.maxY, app.windows.firstMatch.frame.maxY)
        capture("tasks-last-row")
    }

    func testStreakLivesInAccount() {
        let guest = launch("account", extra: ["-streakFixture"])
        XCTAssertTrue(guest.buttons["Sign In"].waitForExistence(timeout: 5))
        XCTAssertFalse(guest.descendants(matching: .any)["streak-summary"].exists)
        returnHome(guest)
        XCTAssertFalse(guest.descendants(matching: .any)["streak"].exists)
        guest.terminate()
        let app = launch("account", extra: ["-streakFixture", "-accountFixture"])
        let streak = app.descendants(matching: .any)["streak-summary"].firstMatch
        XCTAssertTrue(streak.waitForExistence(timeout: 5))
        XCTAssertTrue(streak.label.contains("7 days"), streak.label)
        XCTAssertTrue(app.descendants(matching: .any)["streak-weeks"].firstMatch.exists)
        capture("account-streak")
        returnHome(app)
        XCTAssertTrue(app.descendants(matching: .any)["streak"].firstMatch.exists)
    }

    func testDayHeadingAndTodayFollowThePager() {
        let app = XCUIApplication()
        app.launchArguments = ["-uiTesting", "-manyTasksFixture", "-startTab", "schedule"]
        app.launch()
        let date = app.descendants(matching: .any)["selected-date"].firstMatch
        XCTAssertTrue(date.waitForExistence(timeout: 5))
        let today = date.value as! String
        let pages = app.scrollViews["day-pages"]
        XCTAssertTrue(app.buttons["Today"].isHittable)
        // Schedule carries no Due bar and no streak.
        XCTAssertFalse(app.buttons["Problem Set"].exists)
        XCTAssertFalse(app.descendants(matching: .any)["streak-summary"].exists)
        pages.swipeLeft()
        XCTAssertNotEqual(date.value as? String, today)
        XCTAssertTrue(app.buttons["Today"].isHittable)
        capture("schedule-next-day")
        pages.swipeRight()
        XCTAssertEqual(date.value as? String, today)
        pages.swipeLeft()
        // A slow drag past the midpoint renames the day under the finger.
        let next = date.value as! String
        let start = pages.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.3))
        start.press(forDuration: 0.05, thenDragTo: pages.coordinate(withNormalizedOffset: CGVector(dx: 0.2, dy: 0.3)),
                    withVelocity: .slow, thenHoldForDuration: 0.4)
        XCTAssertNotEqual(date.value as? String, next)
        app.buttons["Today"].tap()
        XCTAssertTrue(app.buttons["Today"].isHittable)
        XCTAssertEqual(date.value as? String, today)
    }

    /// A short pull springs back in either direction.
    func testShortPullSpringsBack() {
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launchArguments = ["-uiTesting", "-manyTasksFixture", "-startTab", "schedule"]
        app.launch()
        let date = app.descendants(matching: .any)["selected-date"].firstMatch
        XCTAssertTrue(date.waitForExistence(timeout: 5))
        let today = date.value as! String
        let pages = app.scrollViews["day-pages"]
        func pull(_ dx: CGFloat) {
            let start = pages.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.3))
            start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: dx, dy: 0)),
                        withVelocity: .slow, thenHoldForDuration: 0.1)
        }
        pull(-35)
        XCTAssertEqual(date.value as? String, today)
        XCTAssertTrue(app.buttons["Today"].isHittable)
        pages.swipeLeft()
        let next = date.value as! String
        XCTAssertNotEqual(next, today)
        pull(35)
        XCTAssertEqual(date.value as? String, next)
        pull(-35)
        XCTAssertEqual(date.value as? String, next)
        capture("short-pull")
    }

    /// With Tasks at Their Due Time, a deadline is a block at its time that opens
    /// the task editor.
    func testTimelineTasksOpenEditor() {
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launchArguments = ["-uiTesting", "-startTab", "account"]
        app.launch()
        let timing = app.switches["Tasks at Their Due Time"]
        XCTAssertTrue(timing.waitForExistence(timeout: 5))
        timing.coordinate(withNormalizedOffset: CGVector(dx: 0.92, dy: 0.5)).tap()
        XCTAssertEqual(timing.value as? String, "1")
        returnHome(app)
        app.buttons["openSchedule"].tap()
        let pages = app.scrollViews["day-pages"]
        XCTAssertTrue(pages.waitForExistence(timeout: 3))
        // The fixture's tasks are due tomorrow at 5 PM.
        pages.swipeLeft()
        let date = app.descendants(matching: .any)["selected-date"].firstMatch
        let timeline = app.scrollViews["day-timeline"]
        let task = app.descendants(matching: .any)["timeline-day-" + (date.value as! String)].firstMatch
            .buttons.matching(NSPredicate(format: "label CONTAINS %@", "Problem Set")).firstMatch
        // Each light flick moves at least two hours, so the afternoon comes up within a few.
        for _ in 0..<6 where !task.isHittable { timeline.swipeUp(velocity: .slow) }
        XCTAssertTrue(task.isHittable)
        capture("task-on-timeline")
        task.tap()
        XCTAssertTrue(app.navigationBars["Edit Task"].waitForExistence(timeout: 3))
        app.buttons["Cancel"].tap()
    }

    func testClassesSearchRemainsVisibleOnOpen() {
        let app = launch("account")
        app.buttons["Find Classes"].tap()
        let search = app.searchFields.firstMatch
        XCTAssertTrue(search.waitForExistence(timeout: 5))
        XCTAssertTrue(search.isHittable)
        let initialY = search.frame.minY
        let list = app.collectionViews["classes-list"]
        XCTAssertTrue(list.waitForExistence(timeout: 3))
        XCTAssertEqual(search.frame.minY, initialY, accuracy: 2)
        capture("classes-search")
        search.tap()
        search.typeText("Math\n")
        XCTAssertEqual(search.value as? String, "Math")
        // The list stays mounted even before the catalog arrives.
        XCTAssertTrue(list.exists)
        XCTAssertTrue(search.isHittable)
    }

    /// Opening a course and coming back lands on the search, and one more
    /// back lands on Account: never straight to Account, never a second
    /// search over the course.
    func testClassSearchOpensCourseAndReturnsOneScreenAtATime() {
        let app = launch("account", extra: ["-catalogFixture"])
        XCTAssertTrue(app.buttons["Find Classes"].waitForExistence(timeout: 5))
        app.buttons["Find Classes"].tap()
        let list = app.collectionViews["classes-list"]
        XCTAssertTrue(list.waitForExistence(timeout: 5))
        XCTAssertTrue(list.staticTexts["PHYS 2514"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.staticTexts["No Results"].exists)

        list.staticTexts["PHYS 2514"].tap()
        // The course and its sections are there the moment the push lands.
        XCTAssertTrue(app.navigationBars["PHYS 2514"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.staticTexts["Section 001"].waitForExistence(timeout: 1))
        XCTAssertTrue(app.buttons["Add section 001"].exists)
        capture("course-detail")

        app.navigationBars["PHYS 2514"].buttons.firstMatch.tap()
        XCTAssertTrue(app.navigationBars["Find Classes"].waitForExistence(timeout: 3))
        XCTAssertTrue(list.staticTexts["PHYS 2514"].waitForExistence(timeout: 3))
        XCTAssertFalse(app.navigationBars["PHYS 2514"].exists)
        XCTAssertFalse(app.navigationBars["Account"].exists)

        // A second course from the same search, then back to Account.
        list.staticTexts["CHEM 1315"].tap()
        XCTAssertTrue(app.navigationBars["CHEM 1315"].waitForExistence(timeout: 3))
        app.navigationBars["CHEM 1315"].buttons.firstMatch.tap()
        XCTAssertTrue(app.navigationBars["Find Classes"].waitForExistence(timeout: 3))
        app.navigationBars["Find Classes"].buttons.firstMatch.tap()
        XCTAssertTrue(app.navigationBars["Account"].waitForExistence(timeout: 3))
        XCTAssertFalse(app.navigationBars["Find Classes"].exists)
    }

    func testFindClassesFromMyClassesReturnsThere() {
        let app = launch("account", extra: ["-catalogFixture"])
        XCTAssertTrue(app.buttons["manageClasses"].waitForExistence(timeout: 5))
        app.buttons["manageClasses"].tap()
        XCTAssertTrue(app.navigationBars["My Classes"].waitForExistence(timeout: 3))
        app.buttons["findClasses"].tap()
        let list = app.collectionViews["classes-list"]
        XCTAssertTrue(list.staticTexts["PHYS 2514"].waitForExistence(timeout: 5))
        list.staticTexts["PHYS 2514"].tap()
        XCTAssertTrue(app.navigationBars["PHYS 2514"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.staticTexts["Section 001"].waitForExistence(timeout: 1))
        app.navigationBars["PHYS 2514"].buttons.firstMatch.tap()
        XCTAssertTrue(app.navigationBars["Find Classes"].waitForExistence(timeout: 3))
        app.navigationBars["Find Classes"].buttons.firstMatch.tap()
        XCTAssertTrue(app.navigationBars["My Classes"].waitForExistence(timeout: 3))
    }

    func testDaySwipeThenVerticalScroll() {
        let app = launch()
        let timeline = app.scrollViews["day-timeline"]
        let date = app.descendants(matching: .any)["selected-date"].firstMatch
        XCTAssertTrue(date.waitForExistence(timeout: 5))
        let previousDate = date.value as? String
        var morning: XCUIElement {
            let currentDate = date.value as! String
            return app.descendants(matching: .any)["timeline-day-" + currentDate].firstMatch
                .buttons.matching(NSPredicate(format: "label CONTAINS %@", "MATH 2924")).firstMatch
        }
        XCTAssertTrue(morning.exists)
        let initialY = morning.frame.minY
        timeline.swipeLeft(velocity: .fast)
        capture("pager-after-left")
        // Do not add an animation delay between changing days and scrolling.
        let nextDate = date.value as? String
        XCTAssertNotEqual(nextDate, previousDate)
        app.scrollViews["day-timeline"].swipeUp(velocity: .fast)
        XCTAssertTrue(app.buttons["Today"].exists)
        XCTAssertEqual(date.value as? String, nextDate)
        XCTAssertTrue(!morning.isHittable || morning.frame.minY < initialY - 40)
        capture("schedule-after-swipes")
        let scrolledY = morning.frame.minY
        timeline.swipeRight(velocity: .slow)
        XCTAssertEqual(date.value as? String, previousDate)
        XCTAssertEqual(morning.frame.minY, scrolledY, accuracy: 3)
        // A short pull springs back without changing dates or the vertical position.
        let start = timeline.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: 35, dy: 0)),
                    withVelocity: .slow, thenHoldForDuration: 0.1)
        XCTAssertEqual(date.value as? String, previousDate)
        XCTAssertEqual(morning.frame.minY, scrolledY, accuracy: 3)
        app.buttons["addButton"].tap()
        XCTAssertTrue(app.navigationBars["New Event"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.descendants(matching: .any)["event-date"].firstMatch.exists)
        capture("new-event")
    }

    func testTimelineFlicksAtLeastTwoHoursAndHoldsWhereDragged() {
        let app = launch()
        let timeline = app.scrollViews["day-timeline"]
        XCTAssertTrue(timeline.waitForExistence(timeout: 5))
        let date = app.descendants(matching: .any)["selected-date"].firstMatch
        func block(_ code: String) -> XCUIElement {
            app.descendants(matching: .any)["timeline-day-" + (date.value as! String)].firstMatch
                .buttons.matching(NSPredicate(format: "label CONTAINS %@", code)).firstMatch
        }
        // The fixture meets at 9:00 and 11:00 every day, and the timeline opens
        // on an hour rule, so hours scrolled are measured from these blocks.
        let math = block("MATH 2924")
        let chem = block("CHEM 1315")
        XCTAssertTrue(math.waitForExistence(timeout: 3))
        let hour = (chem.frame.minY - math.frame.minY) / 2
        let opening = chem.frame.minY
        /// A new gesture can catch a jump mid-flight, so each measurement
        /// waits for the timeline to rest.
        func hoursScrolled() -> CGFloat {
            var last = chem.frame.minY
            for _ in 0..<20 {
                Thread.sleep(forTimeInterval: 0.15)
                let now = chem.frame.minY
                if abs(now - last) < 0.5 { break }
                last = now
            }
            return (opening - chem.frame.minY) / hour
        }
        let middle = timeline.coordinate(withNormalizedOffset: CGVector(dx: 0.6, dy: 0.6))
        /// A light flick: a short stroke that lifts while still moving.
        func flick(_ dy: CGFloat) {
            middle.press(forDuration: 0.05, thenDragTo: middle.withOffset(CGVector(dx: 0, dy: dy)),
                         withVelocity: 400, thenHoldForDuration: 0)
        }

        XCTAssertEqual(hoursScrolled(), 0, accuracy: 0.05)
        // A light flick moves at least two hours and keeps any momentum past
        // that, settling on an hour either way.
        flick(-40)
        let up = hoursScrolled()
        XCTAssertGreaterThanOrEqual(up, 1.95, "a light flick up should move at least two hours")
        XCTAssertEqual(up, up.rounded(), accuracy: 0.05, "a flick should settle on an hour")
        capture("timeline-after-flick")
        flick(40)
        let down = hoursScrolled()
        XCTAssertLessThanOrEqual(down, max(up - 1.95, 0.05), "a light flick down should move back at least two hours")
        XCTAssertEqual(down, down.rounded(), accuracy: 0.05, "a flick should settle on an hour")

        // Held and dragged, the timeline stays where the finger leaves it.
        middle.press(forDuration: 0.1, thenDragTo: middle.withOffset(CGVector(dx: 0, dy: -hour * 1.5)),
                     withVelocity: .slow, thenHoldForDuration: 0.4)
        let held = hoursScrolled()
        XCTAssertGreaterThan(held, 1)
        XCTAssertGreaterThan(abs(held - held.rounded()), 0.2, "a held drag should not snap to an hour")
        flick(-40)
        let after = hoursScrolled()
        XCTAssertGreaterThanOrEqual(after, (held + 2).rounded() - 0.05)
        XCTAssertEqual(after, after.rounded(), accuracy: 0.05)
    }

    func testDraggingScheduleBlocksDoesNotOpenDetails() {
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launchArguments = ["-uiTesting", "-scheduleGestureFixture", "-startTab", "schedule"]
        app.launch()
        let date = app.descendants(matching: .any)["selected-date"].firstMatch
        XCTAssertTrue(date.waitForExistence(timeout: 5))
        let originalDate = date.value as? String
        capture("pager-initial")
        // Re-query after paging: each date has a separate occurrence of the card.
        var classBlock: XCUIElement {
            app.descendants(matching: .any)["timeline-day-" + (date.value as! String)].firstMatch
                .buttons.matching(NSPredicate(format: "label CONTAINS %@", "MATH 2924")).firstMatch
        }
        var eventBlock: XCUIElement {
            app.descendants(matching: .any)["timeline-day-" + (date.value as! String)].firstMatch
                .buttons.matching(NSPredicate(format: "label CONTAINS %@", "Study Session")).firstMatch
        }

        func drag(_ block: XCUIElement, dx: CGFloat, dy: CGFloat = 0) {
            XCTAssertTrue(block.isHittable)
            let start = block.coordinate(withNormalizedOffset: CGVector(dx: dx > 0 ? 0.2 : 0.8, dy: 0.5))
            start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: dx, dy: dy)),
                        withVelocity: .slow, thenHoldForDuration: 0.1)
            XCTAssertFalse(app.navigationBars["MATH 2924"].exists)
            XCTAssertFalse(app.navigationBars["Study Session"].exists)
            XCTAssertTrue(app.buttons["Today"].isHittable)
        }

        // Start on the cards, including a cancelled pull that returns under the finger.
        drag(classBlock, dx: 35)
        XCTAssertEqual(date.value as? String, originalDate)
        drag(classBlock, dx: -220)
        XCTAssertNotEqual(date.value as? String, originalDate)
        drag(eventBlock, dx: 220)
        XCTAssertEqual(date.value as? String, originalDate)
        drag(eventBlock, dx: -35)
        XCTAssertEqual(date.value as? String, originalDate)

        // A deliberate tap still opens each kind of detail after a swipe.
        classBlock.tap()
        XCTAssertTrue(app.navigationBars["MATH 2924"].waitForExistence(timeout: 3))
        app.buttons["Done"].tap()
        eventBlock.tap()
        XCTAssertTrue(app.navigationBars["Study Session"].waitForExistence(timeout: 3))
        app.buttons["Done"].tap()
        let initialY = eventBlock.frame.minY
        drag(eventBlock, dx: 0, dy: -90)
        XCTAssertEqual(date.value as? String, originalDate)
        XCTAssertLessThan(eventBlock.frame.minY, initialY - 40)
        capture("schedule-blocks-after-drags")
    }

    func testNativeDayPagingWithDenseEvents() {
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launchArguments = ["-uiTesting", "-dayPagingFixture", "-startTab", "schedule"]
        app.launch()
        let pages = app.scrollViews["day-pages"]
        XCTAssertTrue(pages.waitForExistence(timeout: 5))
        let date = app.descendants(matching: .any)["selected-date"].firstMatch
        let today = date.value as? String
        func event(_ day: Int) -> XCUIElement {
            app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "Day \(day) Event 0.")).firstMatch
        }
        XCTAssertTrue(event(0).isHittable)
        let originalX = event(0).frame.minX
        // Collect native scrolling metrics with dense, overlapping events.
        let options = XCTMeasureOptions()
        options.iterationCount = 3
        measure(metrics: [XCTOSSignpostMetric.scrollingAndDecelerationMetric], options: options) {
            pages.swipeLeft(velocity: .fast)
            pages.swipeRight(velocity: .fast)
        }
        XCTAssertTrue(event(0).isHittable)
        XCTAssertEqual(event(0).frame.minX, originalX, accuracy: 1)
        capture("dense-pager-today")
        pages.swipeLeft(velocity: .slow)
        XCTAssertTrue(event(1).isHittable)
        XCTAssertFalse(event(0).isHittable)
        XCTAssertEqual(event(1).frame.minX, originalX, accuracy: 1)
        capture("dense-pager-tomorrow")
        pages.swipeRight(velocity: .fast)
        pages.swipeRight(velocity: .slow)
        XCTAssertTrue(event(-1).isHittable)
        XCTAssertFalse(event(1).isHittable)
        XCTAssertEqual(event(-1).frame.minX, originalX, accuracy: 1)
        capture("dense-pager-yesterday")
        app.buttons["Today"].tap()
        XCTAssertEqual(date.value as? String, today)
        XCTAssertTrue(event(0).exists)
        XCTAssertEqual(event(0).frame.minX, originalX, accuracy: 1)
    }

    func testEditorToolbarActions() {
        let app = launch()
        app.buttons["addButton"].tap()
        let eventBar = app.navigationBars["New Event"]
        XCTAssertTrue(eventBar.waitForExistence(timeout: 3))
        XCTAssertFalse(eventBar.buttons["Add"].isEnabled)
        capture("event-toolbar-disabled")
        app.textFields["Title"].tap()
        app.textFields["Title"].typeText("Toolbar Event")
        XCTAssertTrue(eventBar.buttons["Add"].isEnabled)
        capture("event-toolbar-enabled")
        eventBar.buttons["Add"].tap()
        XCTAssertFalse(eventBar.exists)
        returnHome(app)
        let all = app.buttons["allTasks"]
        reveal(all, in: app)
        all.tap()
        XCTAssertTrue(app.scrollViews["tasks-list"].waitForExistence(timeout: 3))
        onscreen(app.buttons.matching(identifier: "addButton")).tap()
        let taskBar = app.navigationBars["New Task"]
        XCTAssertTrue(taskBar.waitForExistence(timeout: 3))
        XCTAssertFalse(taskBar.buttons["Add"].isEnabled)
        app.textFields["Title"].tap()
        app.textFields["Title"].typeText("Toolbar Task")
        XCTAssertTrue(taskBar.buttons["Add"].isEnabled)
        capture("task-toolbar-enabled")
        taskBar.buttons["Add"].tap()
        XCTAssertTrue(app.scrollViews["tasks-list"].buttons["Toolbar Task"].waitForExistence(timeout: 3))
    }

    func testTaskFilterAndAccountNavigation() {
        let app = launch("tasks")
        let list = app.scrollViews["tasks-list"]
        XCTAssertTrue(list.buttons["task-fixture-0"].waitForExistence(timeout: 5))
        XCTAssertTrue(list.buttons["task-fixture-1"].exists)
        capture("tasks")
        list.buttons["task-fixture-1"].tap()
        XCTAssertTrue(app.navigationBars["Edit Task"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.descendants(matching: .any).matching(
            NSPredicate(format: "label == %@", "Open in Canvas")).firstMatch.exists)
        app.buttons["Cancel"].tap()
        // A class chip narrows the list to that class.
        app.buttons["CHEM 1315"].tap()
        XCTAssertTrue(list.buttons["task-fixture-1"].waitForExistence(timeout: 3))
        XCTAssertFalse(list.buttons["task-fixture-0"].exists)
        // A new task keeps the filter: the editor is a sheet over the list.
        onscreen(app.buttons.matching(identifier: "addButton")).tap()
        XCTAssertTrue(app.navigationBars["New Task"].waitForExistence(timeout: 3))
        capture("new-task")
        app.buttons["Cancel"].tap()
        XCTAssertTrue(list.buttons["task-fixture-1"].waitForExistence(timeout: 3))
        XCTAssertFalse(list.buttons["task-fixture-0"].exists)
        returnHome(app)
        app.buttons["accountButton"].tap()
        XCTAssertTrue(app.navigationBars["Account"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.buttons["manageClasses"].exists)
        XCTAssertFalse(app.buttons.matching(identifier: "addButton").allElementsBoundByIndex.contains { $0.isHittable })
        capture("account")
        app.buttons["manageClasses"].tap()
        XCTAssertTrue(app.navigationBars["My Classes"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.buttons["Remove MATH 2924"].exists)
        app.buttons["Remove MATH 2924"].tap()
        XCTAssertFalse(app.buttons["Remove MATH 2924"].exists)
        XCTAssertTrue(app.buttons["Remove CHEM 1315"].exists)
        // The task list opens again from Home.
        returnHome(app)
        let all = app.buttons["allTasks"]
        reveal(all, in: app)
        all.tap()
        XCTAssertTrue(app.scrollViews["tasks-list"].buttons["task-fixture-1"].waitForExistence(timeout: 3))
    }

    func testRefreshKeepsTaskRowsStable() {
        let app = launch("tasks", extra: ["-refreshFixture"])
        let list = app.scrollViews["tasks-list"]
        XCTAssertTrue(list.buttons["task-fixture-0"].waitForExistence(timeout: 5))
        let initialTask = list.buttons["task-fixture-0"].frame
        list.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.4))
            .press(forDuration: 0.1, thenDragTo: list.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.8)))
        // XCTest waits for refresh animations to settle after the drag.
        let finished = XCTNSPredicateExpectation(predicate: NSPredicate(format: "value == %@", "Updated"), object: list)
        XCTAssertEqual(XCTWaiter.wait(for: [finished], timeout: 10), .completed)
        XCTAssertEqual(list.buttons["task-fixture-0"].frame.minY, initialTask.minY, accuracy: 1)
        capture("tasks-after-refresh")
    }

    func testWelcomeSequenceAndPhoneFormatting() {
        let app = launch("home", welcome: true)
        XCTAssertTrue(app.buttons["Get Started"].waitForExistence(timeout: 5))
        capture("welcome")
        app.buttons["Sign In"].tap()
        let phone = app.textFields["Mobile number"]
        XCTAssertTrue(phone.waitForExistence(timeout: 3))
        phone.tap()
        // Separators arrive with the digits.
        phone.typeText("415555")
        XCTAssertTrue(phone.wait(forValue: "415-555"), "value: \(String(describing: phone.value))")
        phone.typeText("0123")
        XCTAssertTrue(phone.wait(forValue: "415-555-0123"), "value: \(String(describing: phone.value))")
        capture("phone-entry")
        app.buttons["Cancel"].tap()
        app.buttons["Get Started"].tap()
        XCTAssertTrue(app.buttons["Test University"].waitForExistence(timeout: 3))
        app.buttons["Test University"].tap()
        XCTAssertTrue(app.staticTexts["Sync Across Devices"].waitForExistence(timeout: 3))
        app.buttons["Continue Without an Account"].tap()
        XCTAssertTrue(app.staticTexts["Import Assignments"].waitForExistence(timeout: 3))
        app.buttons["Skip"].tap()
        // A school without Canvas ends on its class search, or the planner.
        XCTAssertTrue(app.staticTexts["Add Your Classes"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.buttons["Search Classes"].exists)
        app.buttons["Skip"].tap()
        XCTAssertTrue(app.buttons["accountButton"].waitForExistence(timeout: 3))
    }
}

extension XCUIElement {
    /// Whether the element's label comes to contain `text` within a few seconds.
    func wait(forLabelContaining text: String, timeout: TimeInterval = 3) -> Bool {
        let deadline = Date(timeIntervalSinceNow: timeout)
        while Date() < deadline {
            if label.contains(text) { return true }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        }
        return label.contains(text)
    }

    /// Whether the element's value becomes `value` within a few seconds; typed
    /// text is shaped a beat after the keystrokes land.
    func wait(forValue value: String, timeout: TimeInterval = 3) -> Bool {
        let predicate = NSPredicate(format: "value == %@", value)
        let expectation = XCTNSPredicateExpectation(predicate: predicate, object: self)
        return XCTWaiter().wait(for: [expectation], timeout: timeout) == .completed
    }
}
