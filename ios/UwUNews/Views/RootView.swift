import SwiftUI
import UwUNewsCore

struct RootView: View {
    @Environment(AppModel.self) private var model
    @State private var tab: TabKey = .overview
    @State private var opened: String?

    enum TabKey: Hashable { case overview, starred, feeds, settings, search }

    var body: some View {
        @Bindable var model = model
        Group {
            if let error = model.startupError {
                EmptyState(
                    title: String(localized: "UwUNews startet nicht"),
                    message: error,
                    mood: .sad
                )
            } else {
                tabs
            }
        }
        .background(Palette.canvas)
        .overlay(alignment: .top) { ToastView(toast: $model.toast) }
        .sheet(item: Binding(get: { opened.map(OpenedArticle.init) }, set: { opened = $0?.key })) { item in
            NavigationStack { ReaderView(key: item.key, sequence: [item.key]) }
        }
        .onReceive(NotificationCenter.default.publisher(for: .openArticle)) { note in
            opened = note.object as? String
            AppDelegate.pendingArticle = nil
        }
        .onAppear {
            if let key = AppDelegate.pendingArticle {
                opened = key
                AppDelegate.pendingArticle = nil
            }
        }
    }

    private var tabs: some View {
        TabView(selection: $tab) {
            Tab(String(localized: "Übersicht"), systemImage: "newspaper", value: .overview) {
                NavigationStack { OverviewView() }
            }
            Tab(String(localized: "Gemerkt"), systemImage: "star", value: .starred) {
                NavigationStack {
                    ArticleListView(scope: .starred, title: String(localized: "Gemerkt"))
                }
            }
            .badge(model.counts.starred > 0 ? Int(model.counts.starred) : 0)
            Tab(String(localized: "Feeds"), systemImage: "dot.radiowaves.up.forward", value: .feeds) {
                NavigationStack { FeedsView() }
            }
            Tab(String(localized: "Einstellungen"), systemImage: "gearshape", value: .settings) {
                NavigationStack { SettingsView() }
            }
            Tab(value: .search, role: .search) {
                NavigationStack { SearchView() }
            }
        }
        .tabBarMinimizeBehavior(.onScrollDown)
    }
}

private struct OpenedArticle: Identifiable {
    let key: String
    var id: String { key }
}

/// A glass toast dropping in at the top, gone after a few seconds.
struct ToastView: View {
    @Binding var toast: Toast?

    var body: some View {
        if let toast {
            HStack(spacing: 12) {
                Image(systemName: icon(toast.tone))
                    .font(.system(size: 15, weight: .bold))
                    .foregroundStyle(.white)
                    .frame(width: 30, height: 30)
                    .background(color(toast.tone), in: Circle())
                VStack(alignment: .leading, spacing: 2) {
                    Text(toast.title).font(UwUFont.style(.subheadline, weight: .semibold))
                    if let detail = toast.detail {
                        Text(detail)
                            .font(UwUFont.style(.footnote))
                            .foregroundStyle(Palette.muted)
                            .lineLimit(2)
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
            .padding(.horizontal, 16)
            .transition(.move(edge: .top).combined(with: .opacity))
            .onTapGesture { self.toast = nil }
            .task(id: toast.id) {
                try? await Task.sleep(for: .seconds(4))
                withAnimation(.snappy) { self.toast = nil }
            }
            .sensoryFeedback(toast.tone == .danger ? .error : .success, trigger: toast.id)
        }
    }

    private func icon(_ tone: Toast.Tone) -> String {
        switch tone {
        case .info: "info"
        case .success: "checkmark"
        case .danger: "exclamationmark"
        }
    }

    private func color(_ tone: Toast.Tone) -> Color {
        switch tone {
        case .info: Palette.pinkSolid
        case .success: Palette.success
        case .danger: Palette.danger
        }
    }
}
