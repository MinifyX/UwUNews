import SwiftUI
import UwUNewsCore

/// The first tab: everything at a glance, as grouped lists.
struct OverviewView: View {
    @Environment(AppModel.self) private var model
    @State private var sheet: Sheet?

    enum Sheet: Identifiable {
        case addSource, newCollection(filter: Bool)
        var id: String {
            switch self {
            case .addSource: "source"
            case let .newCollection(filter): "collection-\(filter)"
            }
        }
    }

    var body: some View {
        List {
            Section {
                link(.all, String(localized: "Alle Artikel"), symbol: "tray.full", color: Palette.pink, count: model.counts.unread)
                link(.today, String(localized: "Heute"), symbol: "sun.max", color: Palette.accent("amber"), count: model.counts.today)
                link(.starred, String(localized: "Gemerkt"), symbol: "star", color: Palette.accent("violet"), count: model.counts.starred)
            }

            if !model.collections.isEmpty {
                Section(String(localized: "Sammlungen")) {
                    ForEach(model.collections, id: \.id) { collection in
                        NavigationLink {
                            ArticleListView(scope: .collection(id: collection.id), title: collection.name)
                        } label: {
                            Row(
                                title: collection.name,
                                subtitle: collection.subtitle(sources: model.sources),
                                badge: SymbolBadge(symbol: collection.symbol, color: Palette.accent(collection.color)),
                                count: collection.unread,
                                bell: collection.notify
                            )
                        }
                    }
                }
            }

            Section(String(localized: "Quellen")) {
                if model.sources.isEmpty {
                    Button {
                        sheet = .addSource
                    } label: {
                        Label(String(localized: "Erste Quelle hinzufügen"), systemImage: "plus.circle.fill")
                    }
                }
                ForEach(model.sources, id: \.id) { source in
                    NavigationLink {
                        ArticleListView(scope: .source(id: source.id), title: source.title)
                    } label: {
                        Row(
                            title: source.title,
                            subtitle: source.lastError.map { String(localized: "Fehler: \($0)") },
                            badge: SourceBadge(source: source),
                            count: source.unread,
                            bell: source.notify,
                            warning: source.lastError != nil
                        )
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .canvasBackground()
        .navigationTitle(String(localized: "Übersicht"))
        .refreshable { await model.refresh() }
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button { sheet = .addSource } label: {
                        Label(String(localized: "Quelle"), systemImage: "dot.radiowaves.up.forward")
                    }
                    Button { sheet = .newCollection(filter: false) } label: {
                        Label(String(localized: "Sammlung"), systemImage: "folder")
                    }
                    Button { sheet = .newCollection(filter: true) } label: {
                        Label(String(localized: "Filter-Feed"), systemImage: "line.3.horizontal.decrease")
                    }
                } label: {
                    Image(systemName: "plus")
                } primaryAction: {
                    sheet = .addSource
                }
                .accessibilityLabel(String(localized: "Hinzufügen"))
            }
        }
        .overlay {
            if model.sources.isEmpty, model.startupError == nil {
                EmptyState(
                    title: String(localized: "Noch keine Feeds"),
                    message: String(localized: "Füge eine Webseite oder die Adresse eines RSS- oder Atom-Feeds hinzu."),
                    mood: .happy,
                    action: (String(localized: "Quelle hinzufügen"), { sheet = .addSource })
                )
                .padding(.top, 160)
            }
        }
        .sheet(item: $sheet) { sheet in
            switch sheet {
            case .addSource:
                AddSourceSheet()
            case let .newCollection(filter):
                CollectionEditor(collection: nil, startWithFilter: filter)
            }
        }
    }

    private func link(_ scope: Scope, _ title: String, symbol: String, color: Color, count: UInt32) -> some View {
        NavigationLink {
            ArticleListView(scope: scope, title: title)
        } label: {
            Row(title: title, subtitle: nil, badge: SymbolBadge(symbol: symbol, color: color), count: count)
        }
    }
}

/// A list row: a badge, a title, an optional muted line, the unread count.
struct Row<Badge: View>: View {
    let title: String
    let subtitle: String?
    let badge: Badge
    var count: UInt32 = 0
    var bell = false
    var warning = false

    var body: some View {
        HStack(spacing: 12) {
            badge
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                    .font(UwUFont.style(.body, weight: .medium))
                    .foregroundStyle(Palette.ink)
                    .lineLimit(1)
                if let subtitle {
                    Text(subtitle)
                        .font(UwUFont.style(.footnote))
                        .foregroundStyle(warning ? Palette.danger : Palette.muted)
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            if bell {
                Image(systemName: "bell.fill")
                    .font(.system(size: 11))
                    .foregroundStyle(Palette.faint)
                    .accessibilityLabel(String(localized: "Benachrichtigungen an"))
            }
            UnreadCount(count: count)
        }
    }
}

/// A source's initial on a tinted square: feeds have no reliable icon.
struct SourceBadge: View {
    let source: FeedSource
    var size: CGFloat = 30

    var body: some View {
        let letter = source.title.trimmingCharacters(in: .whitespaces).first.map { String($0).uppercased() } ?? "?"
        Text(letter)
            .font(UwUFont.style(.subheadline, weight: .bold))
            .foregroundStyle(Palette.pinkInk)
            .frame(width: size, height: size)
            .background(Palette.pinkTint, in: RoundedRectangle(cornerRadius: size * 0.3, style: .continuous))
    }
}

extension FeedCollection {
    func subtitle(sources: [FeedSource]) -> String {
        let from = allSources
            ? String(localized: "Alle Quellen")
            : String(localized: "\(sourceIds.count) Quellen")
        let words = any.count + all.count + exclude.count
        return words == 0 ? from : from + " · " + String(localized: "\(words) Filterwörter")
    }
}
