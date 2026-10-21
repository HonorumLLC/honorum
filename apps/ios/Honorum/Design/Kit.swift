import CoreText
import SwiftUI
import UIKit

// The pieces every Honorum screen is built from, taken from the web app's
// tokens: Poppins Medium for display type, white cards edged with the web's
// hairline on the warm surface, the three ring colors, and pastel tiles in
// the class palette. `Palette.swift` holds the colors themselves.

// MARK: Type

extension Font {
    /// Poppins Medium, registered from the web's own font file on first use.
    static func display(_ size: CGFloat, relativeTo style: Font.TextStyle = .title) -> Font {
        _ = Typography.registered
        return .custom("Poppins-Medium", size: size, relativeTo: style)
    }
}

enum Typography {
    static let registered: Bool = {
        guard let url = Bundle.main.url(forResource: "Poppins-Medium", withExtension: "ttf") else { return false }
        return CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
    }()

    /// Navigation titles in the display face, so a pushed screen's title
    /// matches the date at the top of Home.
    @MainActor static func install() {
        _ = registered
        guard let large = UIFont(name: "Poppins-Medium", size: 32),
              let inline = UIFont(name: "Poppins-Medium", size: 17) else { return }
        let ink = UIColor(Color.ink)
        let bar = UINavigationBar.appearance()
        bar.largeTitleTextAttributes = [
            .font: UIFontMetrics(forTextStyle: .largeTitle).scaledFont(for: large), .foregroundColor: ink,
        ]
        bar.titleTextAttributes = [
            .font: UIFontMetrics(forTextStyle: .headline).scaledFont(for: inline), .foregroundColor: ink,
        ]
    }
}

// MARK: Rings

extension RingPeriod {
    /// The ring's stroke, as the web draws it: `--ring-day`, `--ring-week`,
    /// and `--ring-term`.
    var color: Color {
        switch self {
        case .today: Color(light: 0xD57A7E, dark: 0xFF7A68)
        case .week: Color(light: 0xF8B585, dark: 0xF0B35A)
        case .term: Color(light: 0x70B1EA, dark: 0x8FB0D0)
        }
    }

    /// The same hue deep enough to carry text on paper.
    var ink: Color {
        switch self {
        case .today: Color(light: 0xB4464D, dark: 0xFF8E7E)
        case .week: Color(light: 0xB0661F, dark: 0xF4C17A)
        case .term: Color(light: 0x2F6EA6, dark: 0xA9C6E2)
        }
    }

    /// The ring's color mixed into the paper, like the web's `--ring-mix`.
    var track: Color { Color.surface.mix(with: color, by: 0.26) }
}

// MARK: Cards and titles

/// A white card on the warm surface, edged with the web's hairline instead
/// of a shadow.
struct CardSurface: ViewModifier {
    var padding: CGFloat = 16

    func body(content: Content) -> some View {
        content
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.card, in: .rect(cornerRadius: 24, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 24, style: .continuous)
                    .strokeBorder(Color.hairline, lineWidth: 1)
            }
    }
}

extension View {
    func card(padding: CGFloat = 16) -> some View { modifier(CardSurface(padding: padding)) }
}

/// A section's title in the display face, with an optional action on the right.
struct SectionTitle: View {
    let title: String
    var detail: String?
    var action: (label: String, id: String, perform: () -> Void)?

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title)
                .font(.display(17, relativeTo: .headline))
                .foregroundStyle(Color.ink)
                .accessibilityAddTraits(.isHeader)
            Spacer()
            if let detail {
                Text(detail)
                    .font(.subheadline)
                    .foregroundStyle(Color.inkMuted)
            }
            if let action {
                Button(action.label, action: action.perform)
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(Color.brandInk)
                    .accessibilityIdentifier(action.id)
            }
        }
        .padding(.horizontal, 4)
    }
}

/// A list section's header, set like `SectionTitle`.
struct ListHeader: View {
    let title: String

    init(_ title: String) { self.title = title }

    var body: some View {
        Text(title)
            .font(.display(16, relativeTo: .headline))
            .foregroundStyle(Color.ink)
            .textCase(nil)
            .accessibilityAddTraits(.isHeader)
    }
}

/// A thin rule between rows, inset to where the row's text begins.
struct RowRule: View {
    var inset: CGFloat = 0

    var body: some View {
        Rectangle().fill(Color.hairline).frame(height: 1).padding(.leading, inset)
    }
}

// MARK: Tiles, chips, and buttons

/// A symbol on a pastel tile in the class palette, the way Settings marks
/// its rows, in Honorum's colors.
struct IconTile: View {
    let symbol: String
    var hex: String = CoursePalette.eventHex
    @ScaledMetric(relativeTo: .body) private var size = 30.0

    init(_ symbol: String, hex: String = CoursePalette.eventHex) {
        self.symbol = symbol
        self.hex = hex
    }

    var body: some View {
        Image(systemName: symbol)
            .font(.system(size: size * 0.5, weight: .semibold))
            .foregroundStyle(CoursePalette.mark(hex))
            .frame(width: size, height: size)
            .background(CoursePalette.fill(hex), in: .rect(cornerRadius: size * 0.3, style: .continuous))
            .accessibilityHidden(true)
    }
}

/// A row's title led by its tile.
struct TileLabel: View {
    let title: String
    let symbol: String
    var hex: String = CoursePalette.eventHex

    init(_ title: String, _ symbol: String, hex: String = CoursePalette.eventHex) {
        self.title = title
        self.symbol = symbol
        self.hex = hex
    }

    var body: some View {
        HStack(spacing: 12) {
            IconTile(symbol, hex: hex)
            Text(title).foregroundStyle(Color.ink)
        }
    }
}

/// The tile colors the menus use, named for what they hold.
enum Tiles {
    static let blue = "#cfe6fb"
    static let pink = "#fbd7d7"
    static let peach = "#fde6c3"
    static let mint = "#d3f0da"
    static let yellow = "#fdf4bb"
    static let lavender = "#e5d9fb"
    static let apricot = "#ffd8c4"
    static let periwinkle = "#d8e1fb"
    static let neutral = CoursePalette.eventHex
}

/// One choice in a row of choices: a capsule that takes the class's pastel
/// and an edge in its tone once chosen.
struct Chip: View {
    let title: String
    var hex: String?
    let selected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                if let hex {
                    Circle().fill(CoursePalette.tone(hex)).frame(width: 8, height: 8)
                }
                Text(title)
                    .font(.subheadline.weight(selected ? .semibold : .regular))
                    .foregroundStyle(selected && hex != nil ? Color.eventInk : selected ? Color.onBrand : Color.ink)
                    .lineLimit(1)
            }
            .padding(.horizontal, 14)
            .frame(minHeight: 36)
            .background {
                Capsule().fill(selected ? (hex.map(CoursePalette.fill) ?? Color.brand) : Color.card)
            }
            .overlay {
                Capsule().strokeBorder(selected ? (hex.map(CoursePalette.tone) ?? Color.brand) : Color.hairline,
                                       lineWidth: selected ? 1.5 : 1)
            }
            .contentShape(.capsule)
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

/// A full-width capsule: brand-filled for the step forward, a soft tint for
/// the other way.
struct PillButtonStyle: ButtonStyle {
    var prominent = true

    func makeBody(configuration: Configuration) -> some View {
        PillBody(configuration: configuration, prominent: prominent)
    }

    private struct PillBody: View {
        let configuration: Configuration
        let prominent: Bool
        @Environment(\.isEnabled) private var enabled

        var body: some View {
            configuration.label
                .font(.body.weight(.semibold))
                .foregroundStyle(prominent ? Color.onBrand : Color.brandInk)
                .frame(maxWidth: .infinity, minHeight: 50)
                .padding(.horizontal, 20)
                .background(prominent ? Color.brand : Color.tintFill, in: .capsule)
                .opacity(enabled ? (configuration.isPressed ? 0.82 : 1) : 0.45)
                .contentShape(.capsule)
        }
    }
}

extension ButtonStyle where Self == PillButtonStyle {
    static var pill: PillButtonStyle { PillButtonStyle() }
    static var softPill: PillButtonStyle { PillButtonStyle(prominent: false) }
}

// MARK: Glass

extension View {
    /// Liquid Glass on iOS 26; a material on iOS 18.
    @ViewBuilder
    func glassSurface<S: Shape>(tint: Color? = nil, in shape: S) -> some View {
        if #available(iOS 26.0, *) {
            glassEffect(.regular.tint(tint).interactive(), in: shape)
        } else {
            background(tint.map { AnyShapeStyle($0) } ?? AnyShapeStyle(.ultraThinMaterial), in: shape)
        }
    }
}

/// The floating bar the main screens share in place of a tab bar: a wide
/// button that adds a task or an event, and, on Home, search.
struct ActionBar: View {
    var title = "New"
    let onNew: () -> Void
    var onSearch: (() -> Void)?

    var body: some View {
        container {
            HStack(spacing: 10) {
                Button(action: onNew) {
                    Label(title, systemImage: "plus")
                        .font(.body.weight(.semibold))
                        .foregroundStyle(Color.onBrand)
                        .frame(maxWidth: .infinity)
                        .frame(height: 50)
                        .glassSurface(tint: .brand, in: Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("addButton")
                if let onSearch {
                    Button(action: onSearch) {
                        Image(systemName: "magnifyingglass")
                            .font(.system(size: 18, weight: .medium))
                            .foregroundStyle(Color.ink)
                            .frame(width: 50, height: 50)
                            .glassSurface(in: Circle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Search Tasks")
                    .accessibilityIdentifier("searchButton")
                }
            }
        }
        .frame(maxWidth: 640)
        .padding(.horizontal, 20)
        .padding(.bottom, 4)
    }

    @ViewBuilder private func container<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        if #available(iOS 26.0, *) {
            GlassEffectContainer(spacing: 10) { content() }
        } else {
            content()
        }
    }
}

extension View {
    /// The action bar floats over the bottom edge, which softens the rows
    /// passing beneath it on iOS 26.
    @ViewBuilder
    func actionBar(title: String = "New", onNew: @escaping () -> Void, onSearch: (() -> Void)? = nil) -> some View {
        let bar = ActionBar(title: title, onNew: onNew, onSearch: onSearch)
        // The bar stays at the bottom, under the keyboard, rather than
        // riding up on it: nothing it does needs the keyboard, and a screen
        // being pushed over it can raise the keyboard while it is still in view.
        if #available(iOS 26.0, *) {
            safeAreaBar(edge: .bottom, spacing: 0) { bar }
                .scrollEdgeEffectStyle(.soft, for: .bottom)
                .ignoresSafeArea(.keyboard, edges: .bottom)
        } else {
            safeAreaInset(edge: .bottom, spacing: 0) { bar }
                .ignoresSafeArea(.keyboard, edges: .bottom)
        }
    }
}
