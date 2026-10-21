import SwiftUI

/// The Honorum palette, carried over from the web app: warm neutrals, a brown
/// accent, and pastel course fills that keep dark ink in both appearances.
///
/// Colors resolve through `UIColor`'s dynamic provider so a single value serves
/// light and dark mode and updates without re-rendering a view hierarchy.
extension Color {
    // Brand
    static let brand = Color(light: 0x8A5C3C, dark: 0xC99F7C)
    static let brandInk = Color(light: 0x7A5033, dark: 0xDDB996)

    // Surfaces
    static let surface = Color(light: 0xF3EDE6, dark: 0x1E1A17)
    static let content = Color(light: 0xFFFDFA, dark: 0x262220)
    static let card = Color(light: 0xFFFFFF, dark: 0x2E2926)
    static let tintFill = Color(light: 0xF0E3D6, dark: 0x3D3128)

    // Ink
    static let ink = Color(light: 0x2A221D, dark: 0xECE5DE)
    static let inkSecondary = Color(light: 0x5F544B, dark: 0xBFB4A9)
    /// Caption ink. The light value clears 4.5:1 on both the surface and the
    /// content color, so hour labels, due dates, and weekday initials stay legible.
    static let inkMuted = Color(light: 0x75695F, dark: 0x948A80)
    /// White on dark chrome, with dark ink for contrast in light mode.
    static let toolbarIcon = Color(light: 0x2A221D, dark: 0xFFFFFF)

    // Lines and accents
    static let hairline = Color(light: 0xE4DBD0, dark: 0x3B3430)
    static let nowLine = Color(light: 0xD9402F, dark: 0xFF7A68)
    static let danger = Color(light: 0xB3261E, dark: 0xF09A8F)
    static let success = Color(light: 0x2E7D4F, dark: 0x7FD19A)
    /// The primary schedule's star.
    static let flame = Color(light: 0xF07A1A, dark: 0xFF9A3C)
    /// The streak flame's gradient, base to tip, and its lighter core; the
    /// web app's `--flame`, `--flame-tip`, and `--flame-core`.
    static let flameBase = Color(light: 0xE8492B, dark: 0xFF6F52)
    static let flameTip = Color(light: 0xF7A431, dark: 0xFFB347)
    static let flameCore = Color(light: 0xFFD98C, dark: 0xFFE3A6)
    /// Text and spinners on a brand-filled button: white on the brown, dark
    /// ink on the tan the dark appearance uses.
    static let onBrand = Color(light: 0xFFFFFF, dark: 0x1E1A17)
    /// Text on the now line's pill: white on the light red, dark ink on the
    /// pale dark-mode red.
    static let onNowLine = Color(light: 0xFFFFFF, dark: 0x1C1614)

    /// Text drawn on top of a pastel course fill. Both appearances stay dark,
    /// because the fill itself is the thing that changes brightness.
    static let eventInk = Color(light: 0x241D18, dark: 0x1C1614)
    static let eventInkSecondary = Color(light: 0x514740, dark: 0x3F3530)
}

extension Color {
    init(light: UInt32, dark: UInt32) {
        self.init(uiColor: UIColor { traits in
            UIColor(rgb: traits.userInterfaceStyle == .dark ? dark : light)
        })
    }
}

extension UIColor {
    fileprivate convenience init(rgb: UInt32) {
        self.init(
            red: CGFloat((rgb >> 16) & 0xFF) / 255,
            green: CGFloat((rgb >> 8) & 0xFF) / 255,
            blue: CGFloat(rgb & 0xFF) / 255,
            alpha: 1)
    }
}

/// Pastel fills for saved classes, in the web app's order and stored as the
/// same hex strings, so a schedule keeps its colors on both clients. Dark mode
/// dims and slightly saturates each fill, matching the web app's
/// `brightness(.82) saturate(1.1)` treatment.
enum CoursePalette {
    static let hexes = [
        "#cfe6fb", "#fbd7d7", "#fde6c3", "#d3f0da", "#fdf4bb", "#e5d9fb", "#ffd8c4", "#d8e1fb",
    ]

    /// Personal events use the neutral fill the web app gives them.
    static let eventHex = "#e6e2dc"

    /// What VoiceOver calls each fill, in the same order.
    static let names = ["Blue", "Pink", "Peach", "Mint", "Yellow", "Lavender", "Apricot", "Periwinkle"]

    static func fill(_ hex: String?) -> Color {
        let rgb = value(of: hex ?? eventHex) ?? value(of: eventHex)!
        return Color(uiColor: UIColor { traits in
            let base = UIColor(rgb: rgb)
            guard traits.userInterfaceStyle == .dark else { return base }
            return base.adjusted(brightness: 0.82, saturation: 1.1)
        })
    }

    /// A class's deeper tone, for the marks drawn beside its pastel fill: a
    /// task's tick circle, mostly. It keeps the fill's hue at a strength that
    /// reads on the warm surfaces in both appearances; the neutral event
    /// fill becomes a plain grey.
    static func tone(_ hex: String?) -> Color {
        let rgb = value(of: hex ?? eventHex) ?? value(of: eventHex)!
        return Color(uiColor: UIColor { traits in
            var h: CGFloat = 0, s: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
            UIColor(rgb: rgb).getHue(&h, saturation: &s, brightness: &b, alpha: &a)
            let dark = traits.userInterfaceStyle == .dark
            if s < 0.08 { return UIColor(white: dark ? 0.62 : 0.5, alpha: 1) }
            return UIColor(hue: h, saturation: min(1, s * (dark ? 3.0 : 3.6)),
                           brightness: dark ? 0.92 : b * 0.66, alpha: 1)
        })
    }

    /// A symbol or label drawn on the class's fill: its tone in light mode,
    /// and the event ink in dark, where the dimmed fill is still light enough
    /// that the brighter tone would wash out on it.
    static func mark(_ hex: String?) -> Color {
        let tone = UIColor(tone(hex))
        let ink = UIColor(Color.eventInkSecondary)
        return Color(uiColor: UIColor { traits in
            traits.userInterfaceStyle == .dark ? ink.resolvedColor(with: traits) : tone.resolvedColor(with: traits)
        })
    }

    private static func value(of hex: String) -> UInt32? {
        var text = Substring(hex)
        if text.hasPrefix("#") { text = text.dropFirst() }
        guard text.count == 6 else { return nil }
        return UInt32(text, radix: 16)
    }
}

extension UIColor {
    fileprivate func adjusted(brightness: CGFloat, saturation: CGFloat) -> UIColor {
        var h: CGFloat = 0, s: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
        guard getHue(&h, saturation: &s, brightness: &b, alpha: &a) else { return self }
        return UIColor(hue: h, saturation: min(s * saturation, 1), brightness: b * brightness, alpha: a)
    }
}
