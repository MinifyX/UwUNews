import SwiftUI
import UIKit

/// The UwUSuite tokens (UwUSuite-Design `tokens.css`) as dynamic colours.
/// Pink marks selection, focus and the main action, never "ok".
enum Palette {
    static let canvas = dynamic(0xF8F4F6, 0x141016)
    static let surface = dynamic(0xFFFFFF, 0x1C171F)
    static let elevated = dynamic(0xFCF8FA, 0x241E28)
    static let ink = dynamic(0x1C1420, 0xF8F2F6)
    static let muted = dynamic(0x716672, 0xB3A8B3)
    static let faint = dynamic(0xA69BA5, 0x7D717D)
    static let hairline = dynamic(0xF2E8EE, 0x2C2430)
    static let border = dynamic(0xE9DDE4, 0x3A3040)
    static let pink = dynamic(0xFF4D8D, 0xFF7FAC)
    static let pinkSolid = dynamic(0xE11D74, 0xFF7FAC)
    static let pinkInk = dynamic(0xA3154F, 0xFFA3C4)
    static let pinkTint = dynamic(0xFFE4EF, 0x3A1A2A)
    static let success = dynamic(0x12A06D, 0x34C78F)
    static let warning = dynamic(0xB86E00, 0xF0A33A)
    static let danger = dynamic(0xD92D44, 0xFF6B7D)

    /// The account colours, offered for collections. The name is what syncs.
    static let accents: [(name: String, color: Color)] = [
        ("pink", dynamic(0xFF4D8D, 0xFF7FAC)),
        ("violet", dynamic(0x8B5CF6, 0xB79CFF)),
        ("sky", dynamic(0x0EA5E9, 0x6CCBF5)),
        ("mint", dynamic(0x10B981, 0x5EDBB0)),
        ("amber", dynamic(0xF59E0B, 0xFFC560)),
        ("coral", dynamic(0xF97362, 0xFFA092)),
    ]

    static func accent(_ name: String) -> Color {
        accents.first { $0.name == name }?.color ?? pink
    }

    private static func dynamic(_ light: UInt32, _ dark: UInt32) -> Color {
        Color(UIColor { traits in
            UIColor(hex: traits.userInterfaceStyle == .dark ? dark : light)
        })
    }
}

extension UIColor {
    convenience init(hex: UInt32) {
        self.init(
            red: CGFloat((hex >> 16) & 0xFF) / 255,
            green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255,
            alpha: 1
        )
    }
}

/// UwU Sans, the suite's interface font (variable, 200–800), scaled with
/// Dynamic Type like the system styles it stands in for.
enum UwUFont {
    static let family = "UwU Sans"

    static func style(_ style: Font.TextStyle, weight: Font.Weight = .regular) -> Font {
        Font.custom(family, size: size(of: style), relativeTo: style).weight(weight)
    }

    private static func size(of style: Font.TextStyle) -> CGFloat {
        switch style {
        case .largeTitle: 34
        case .title: 28
        case .title2: 22
        case .title3: 20
        case .headline: 17
        case .body: 17
        case .callout: 16
        case .subheadline: 15
        case .footnote: 13
        case .caption: 12
        case .caption2: 11
        default: 17
        }
    }

    /// Navigation bar titles in UwU Sans too.
    @MainActor static func applyToBars() {
        let large = UIFont(name: "UwUSans-Regular", size: 34).map {
            UIFontMetrics(forTextStyle: .largeTitle).scaledFont(for: $0.withWeight(.heavy))
        }
        let inline = UIFont(name: "UwUSans-Regular", size: 17).map {
            UIFontMetrics(forTextStyle: .headline).scaledFont(for: $0.withWeight(.bold))
        }
        let appearance = UINavigationBar.appearance()
        if let large {
            appearance.largeTitleTextAttributes = [.font: large]
        }
        if let inline {
            appearance.titleTextAttributes = [.font: inline]
        }
    }
}

private extension UIFont {
    /// A weight of a variable font, through its `wght` axis.
    func withWeight(_ weight: UIFont.Weight) -> UIFont {
        let wght: CGFloat = switch weight {
        case .heavy: 760
        case .bold: 700
        case .semibold: 600
        case .medium: 500
        default: 400
        }
        // 'wght' as the four-character code the variation dictionary is keyed by.
        let axis = 0x7767_6874
        let descriptor = fontDescriptor.addingAttributes([
            UIFontDescriptor.AttributeName(rawValue: kCTFontVariationAttribute as String): [axis: wght],
        ])
        return UIFont(descriptor: descriptor, size: pointSize)
    }
}

extension View {
    /// The suite's canvas behind a list or a page.
    func canvasBackground() -> some View {
        scrollContentBackground(.hidden).background(Palette.canvas)
    }
}

/// A round tinted badge with a symbol, as on collection rows.
struct SymbolBadge: View {
    let symbol: String
    var color: Color = Palette.pink
    var size: CGFloat = 30

    var body: some View {
        Image(systemName: symbol)
            .font(.system(size: size * 0.48, weight: .semibold))
            .foregroundStyle(.white)
            .frame(width: size, height: size)
            .background(color.gradient, in: RoundedRectangle(cornerRadius: size * 0.3, style: .continuous))
    }
}

/// The count of unread articles at the end of a row.
struct UnreadCount: View {
    let count: UInt32

    var body: some View {
        if count > 0 {
            Text(count > 999 ? "999+" : "\(count)")
                .font(UwUFont.style(.subheadline, weight: .medium))
                .monospacedDigit()
                .foregroundStyle(Palette.muted)
        }
    }
}
