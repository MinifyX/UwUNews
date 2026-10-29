import SwiftUI

/// Nyu with a line that says what to do, for empty lists and failures.
struct EmptyState: View {
    enum Mood: String { case happy, sleepy, sad }

    let title: String
    let message: String
    var mood: Mood = .happy
    var action: (String, () -> Void)?

    var body: some View {
        VStack(spacing: 14) {
            Image("Nyu-\(mood.rawValue)")
                .resizable()
                .scaledToFit()
                .frame(width: 120, height: 120)
                .accessibilityHidden(true)
            Text(title)
                .font(UwUFont.style(.title3, weight: .bold))
                .foregroundStyle(Palette.ink)
            Text(message)
                .font(UwUFont.style(.subheadline))
                .foregroundStyle(Palette.muted)
                .multilineTextAlignment(.center)
            if let action {
                Button(action.0, action: action.1)
                    .buttonStyle(.glassProminent)
                    .padding(.top, 4)
            }
        }
        .padding(32)
        .frame(maxWidth: 360)
    }
}
