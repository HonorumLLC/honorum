import SwiftUI

// The term's pieces on Home: its arc with what is ahead, and a tile per class.

/// The term from its first day to its last, filled to today and labeled with
/// the week, then what is ahead in it.
struct TermCard: View {
    let term: HomeTerm
    let milestones: [HomeMilestone]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            GeometryReader { proxy in
                let x = { (date: CivilDate) in CGFloat(term.position(date)) * proxy.size.width }
                ZStack(alignment: .topLeading) {
                    Capsule().fill(RingPeriod.term.track).frame(height: 8)
                    Capsule().fill(RingPeriod.term.color).frame(width: max(8, x(term.today)), height: 8)
                    ForEach(milestones) { milestone in
                        Circle()
                            .fill(milestone.colorHex.map(CoursePalette.tone) ?? Color.inkMuted)
                            .overlay { Circle().strokeBorder(Color.card, lineWidth: 2) }
                            .frame(width: 12, height: 12)
                            .position(x: x(milestone.from), y: 4)
                    }
                    Text("Week \(term.week)")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(RingPeriod.term.ink)
                        .fixedSize()
                        .position(x: min(max(x(term.today), 40), proxy.size.width - 40), y: 24)
                    HStack {
                        Text(term.start.monthDay)
                        Spacer()
                        Text(term.end.monthDay)
                    }
                    .font(.caption)
                    .foregroundStyle(Color.inkMuted)
                    .offset(y: 16)
                }
            }
            .frame(height: 34)
            .padding(16)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Week \(term.week) of \(term.weeks)")
            if !milestones.isEmpty {
                RowRule()
                ForEach(Array(milestones.prefix(3).enumerated()), id: \.element.id) { index, milestone in
                    if index > 0 { RowRule(inset: 34) }
                    HStack(spacing: 10) {
                        Circle()
                            .fill(milestone.colorHex.map(CoursePalette.tone) ?? Color.inkMuted)
                            .frame(width: 8, height: 8)
                            .frame(width: 14)
                        Text(milestone.title)
                            .font(.body)
                            .foregroundStyle(Color.ink)
                        Spacer(minLength: 8)
                        Text(milestone.dateLabel)
                            .font(.subheadline)
                            .foregroundStyle(Color.inkMuted)
                    }
                    .padding(.horizontal, 16)
                    .frame(minHeight: 46)
                    .accessibilityElement(children: .combine)
                }
            }
        }
        .card(padding: 0)
    }
}

/// Each class as a tile in its own color, with its share of the term as a ring.
struct ClassTiles: View {
    let courses: [HomeCourseItem]

    var body: some View {
        LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)], spacing: 10) {
            ForEach(Array(courses.enumerated()), id: \.element.id) { index, course in
                VStack(alignment: .leading, spacing: 12) {
                    ZStack {
                        ProgressRing(fraction: course.count.fraction, color: CoursePalette.tone(course.colorHex),
                                     lineWidth: 5, delay: 0.08 * Double(index))
                        Text(course.count.total == 0 ? "–" : "\(course.count.percent)")
                            .font(.system(size: 12, weight: .bold))
                            .monospacedDigit()
                            .foregroundStyle(Color.eventInk)
                    }
                    .frame(width: 44, height: 44)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(course.title)
                            .font(.headline)
                            .foregroundStyle(Color.eventInk)
                            .lineLimit(2, reservesSpace: true)
                        Text(course.next.map { "Next: \($0)" } ?? "Nothing due")
                            .font(.footnote)
                            .foregroundStyle(Color.eventInkSecondary)
                            .lineLimit(1)
                    }
                }
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(CoursePalette.fill(course.colorHex), in: .rect(cornerRadius: 22, style: .continuous))
                .accessibilityElement(children: .combine)
                .accessibilityLabel("\(course.title), \(course.count.done) of \(course.count.total) done")
            }
        }
    }
}
