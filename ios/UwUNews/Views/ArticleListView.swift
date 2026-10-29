import SwiftUI
import UwUNewsCore

/// Articles of one scope, newest first, loaded a page at a time.
struct ArticleListView: View {
    @Environment(AppModel.self) private var model
    @AppStorage("unreadOnly") private var unreadOnly = false
    let scope: Scope
    let title: String

    @State private var articles: [Article] = []
    @State private var loaded = false
    @State private var exhausted = false
    @State private var editing: EditTarget?

    private static let page: UInt32 = 60

    enum EditTarget: Identifiable {
        case source(FeedSource), collection(FeedCollection)
        var id: String {
            switch self {
            case let .source(s): "s" + s.id
            case let .collection(c): "c" + c.id
            }
        }
    }

    var body: some View {
        List {
            ForEach(articles, id: \.key) { article in
                NavigationLink {
                    ReaderView(key: article.key, sequence: articles.map(\.key))
                } label: {
                    ArticleRow(article: article, showSource: !isSource)
                }
                .swipeActions(edge: .leading) {
                    Button {
                        Task { await model.setRead([article.key], !article.read) }
                    } label: {
                        Label(
                            article.read ? String(localized: "Ungelesen") : String(localized: "Gelesen"),
                            systemImage: article.read ? "circle.fill" : "checkmark.circle"
                        )
                    }
                    .tint(Palette.accent("sky"))
                }
                .swipeActions(edge: .trailing) {
                    Button {
                        Task { await model.setStarred(article.key, !article.starred) }
                    } label: {
                        Label(
                            article.starred ? String(localized: "Nicht mehr merken") : String(localized: "Merken"),
                            systemImage: article.starred ? "star.slash" : "star"
                        )
                    }
                    .tint(Palette.accent("amber"))
                }
                .contextMenu { ArticleMenu(article: article) }
                .onAppear {
                    if article.key == articles.last?.key { Task { await loadMore() } }
                }
            }
        }
        .listStyle(.plain)
        .canvasBackground()
        .navigationTitle(title)
        .refreshable { await model.refresh(source: sourceId) }
        .overlay {
            if loaded, articles.isEmpty {
                EmptyState(
                    title: emptyTitle,
                    message: emptyMessage,
                    mood: scope.isStarred ? .sleepy : .happy
                )
            }
        }
        .toolbar {
            ToolbarItemGroup(placement: .topBarTrailing) {
                if !scope.isStarred {
                    Button {
                        unreadOnly.toggle()
                    } label: {
                        Image(systemName: unreadOnly
                            ? "line.3.horizontal.decrease.circle.fill"
                            : "line.3.horizontal.decrease.circle")
                    }
                    .accessibilityLabel(String(localized: "Nur ungelesene"))
                }
                Menu {
                    if !scope.isStarred {
                        Button {
                            Task { await model.markAllRead(scope) }
                        } label: {
                            Label(String(localized: "Alle als gelesen markieren"), systemImage: "checkmark.circle")
                        }
                    }
                    if let edit = editTarget {
                        Button {
                            editing = edit
                        } label: {
                            Label(String(localized: "Bearbeiten"), systemImage: "pencil")
                        }
                    }
                } label: {
                    Image(systemName: "ellipsis")
                }
            }
        }
        .sheet(item: $editing) { target in
            switch target {
            case let .source(source): SourceEditor(source: source)
            case let .collection(collection): CollectionEditor(collection: collection)
            }
        }
        .task(id: "\(model.revision)-\(unreadOnly)") { await reload() }
    }

    private var isSource: Bool {
        if case .source = scope { return true }
        return false
    }

    private var sourceId: String? {
        if case let .source(id) = scope { return id }
        return nil
    }

    private var editTarget: EditTarget? {
        switch scope {
        case let .source(id): model.source(id).map(EditTarget.source)
        case let .collection(id): model.collection(id).map(EditTarget.collection)
        default: nil
        }
    }

    private var emptyTitle: String {
        if scope.isStarred { return String(localized: "Nichts gemerkt") }
        return unreadOnly ? String(localized: "Alles gelesen") : String(localized: "Noch keine Artikel")
    }

    private var emptyMessage: String {
        if scope.isStarred {
            return String(localized: "Wische einen Artikel nach links, um ihn für später zu merken.")
        }
        return unreadOnly
            ? String(localized: "Neue Artikel erscheinen hier, sobald sie veröffentlicht werden.")
            : String(localized: "Zum Aktualisieren nach unten ziehen.")
    }

    private func query(after last: Article?) -> ArticleQuery {
        ArticleQuery(
            scope: scope,
            unreadOnly: unreadOnly && !scope.isStarred,
            search: nil,
            limit: Self.page,
            before: last?.sortAt,
            beforeKey: last?.key
        )
    }

    private func reload() async {
        // Keep what is loaded: after marking one read the list should not jump back to the top.
        let count = max(UInt32(articles.count), Self.page)
        var q = query(after: nil)
        q.limit = count
        articles = await model.articles(q)
        exhausted = articles.count < Int(count)
        loaded = true
    }

    private func loadMore() async {
        guard !exhausted, let last = articles.last else { return }
        let more = await model.articles(query(after: last))
        exhausted = more.count < Int(Self.page)
        let known = Set(articles.map(\.key))
        articles += more.filter { !known.contains($0.key) }
    }
}

struct ArticleRow: View {
    let article: Article
    var showSource = true

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    if !article.read {
                        Circle().fill(Palette.pinkSolid).frame(width: 7, height: 7)
                            .accessibilityLabel(String(localized: "Ungelesen"))
                    }
                    if showSource {
                        Text(article.sourceTitle).lineLimit(1)
                        Text("·")
                    }
                    Text(article.date, format: .relative(presentation: .named, unitsStyle: .abbreviated))
                    if article.starred {
                        Image(systemName: "star.fill").foregroundStyle(Palette.accent("amber"))
                    }
                }
                .font(UwUFont.style(.caption, weight: .medium))
                .foregroundStyle(Palette.muted)

                Text(article.title)
                    .font(UwUFont.style(.headline, weight: article.read ? .medium : .bold))
                    .foregroundStyle(article.read ? Palette.muted : Palette.ink)
                    .lineLimit(3)

                if !article.summary.isEmpty {
                    Text(article.summary)
                        .font(UwUFont.style(.subheadline))
                        .foregroundStyle(Palette.muted)
                        .lineLimit(2)
                }
            }
            Spacer(minLength: 0)
            if let image = article.imageUrl.flatMap(URL.init(string:)) {
                AsyncImage(url: image, transaction: Transaction(animation: .easeOut)) { phase in
                    if let picture = phase.image {
                        picture.resizable().scaledToFill()
                    } else {
                        Palette.hairline
                    }
                }
                .frame(width: 76, height: 76)
                .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
                .opacity(article.read ? 0.7 : 1)
            }
        }
        .padding(.vertical, 6)
        .listRowBackground(Palette.canvas)
        .listRowSeparatorTint(Palette.hairline)
    }
}

struct ArticleMenu: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openURL) private var openURL
    let article: Article

    var body: some View {
        Button {
            Task { await model.setRead([article.key], !article.read) }
        } label: {
            Label(
                article.read ? String(localized: "Als ungelesen markieren") : String(localized: "Als gelesen markieren"),
                systemImage: article.read ? "circle.fill" : "checkmark.circle"
            )
        }
        Button {
            Task { await model.setStarred(article.key, !article.starred) }
        } label: {
            Label(
                article.starred ? String(localized: "Nicht mehr merken") : String(localized: "Merken"),
                systemImage: article.starred ? "star.slash" : "star"
            )
        }
        if let link = article.link.flatMap(URL.init(string:)) {
            Button {
                openURL(link)
            } label: {
                Label(String(localized: "Im Browser öffnen"), systemImage: "safari")
            }
            ShareLink(item: link) {
                Label(String(localized: "Teilen"), systemImage: "square.and.arrow.up")
            }
        }
    }
}

extension Article {
    var date: Date { Date(timeIntervalSince1970: TimeInterval(sortAt) / 1000) }
}
