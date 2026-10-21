import Testing

@testable import Honorum

@MainActor
@Suite("The day the schedule's chrome shows")
struct PagerProgressTests {
    @Test("Before the pager reports, the chrome keeps the selection")
    func unreported() {
        let progress = PagerProgress()
        #expect(progress.displayedDay == nil)
        #expect(progress.restingDay == nil)
    }

    @Test("A swipe changes the displayed day at the midpoint, and the resting day only at rest")
    func swipe() {
        let progress = PagerProgress()
        progress.settle(on: 100)
        progress.follow(page: 100.49)
        #expect(progress.displayedDay == 100)
        progress.follow(page: 100.51)
        #expect(progress.displayedDay == 101)
        #expect(progress.restingDay == 100)
        progress.settle(on: 101)
        #expect(progress.displayedDay == 101)
        #expect(progress.restingDay == 101)
    }

    @Test("A second swipe before the first rests still shows the day on screen")
    func swipeAgainBeforeRest() {
        let progress = PagerProgress()
        progress.settle(on: 100)
        progress.follow(page: 100.9)
        progress.follow(page: 101.6)
        #expect(progress.displayedDay == 102)
        #expect(progress.restingDay == 100)
    }

    @Test("A pull that springs back changes nothing")
    func shortPull() {
        let progress = PagerProgress()
        progress.settle(on: 100)
        progress.follow(page: 100.2)
        progress.follow(page: 100)
        progress.settle(on: 100)
        #expect(progress.displayedDay == 100)
        #expect(progress.restingDay == 100)
    }

    @Test("A jump shows its destination at once and ignores the days it passes")
    func jump() {
        let progress = PagerProgress()
        progress.settle(on: 100)
        progress.jump(to: 104)
        #expect(progress.displayedDay == 104)
        #expect(progress.restingDay == 104)
        progress.follow(page: 101.7)
        progress.follow(page: 103.2)
        #expect(progress.displayedDay == 104)
        progress.settle(on: 104)
        progress.follow(page: 103.4)
        #expect(progress.displayedDay == 103)
    }

    @Test("A finger that takes hold mid-jump is followed again")
    func interruptedJump() {
        let progress = PagerProgress()
        progress.settle(on: 100)
        progress.jump(to: 104)
        progress.interrupt()
        progress.follow(page: 102.2)
        #expect(progress.displayedDay == 102)
        // The bar keeps the height of the destination until the pager rests.
        #expect(progress.restingDay == 104)
        progress.settle(on: 102)
        #expect(progress.restingDay == 102)
    }
}
