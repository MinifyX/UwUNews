import SwiftUI
import UwUNewsCore

/// Managing what is followed: collections in their order, sources with
/// their bells.
struct FeedsView: View {
    @Environment(AppModel.self) private var model
    @State private var editingSource: FeedSource?
    @State private var editingCollection: FeedCollection?
    @State private var newCollection: Bool?
    @State private var adding = false

    var body: some View {
        List {
            Section {
                ForEach(model.collections, id: \.id) { collection in
                    Button {
                        editingCollection = collection
                    } label: {
                        Row(
                            title: collection.name,
                            subtitle: collection.subtitle(sources: model.sources),
                            badge: SymbolBadge(symbol: collection.symbol, color: Palette.accent(collection.color)),
                            bell: collection.notify
                        )
                    }
                    .swipeActions {
                        bellButton(on: collection.notify) {
                            var changed = collection
                            changed.notify.toggle()
                            if changed.notify { _ = await Notifier.requestPermission() }
                            await model.save(changed)
                        }
                    }
                }
                .onMove { from, to in
                    var ids = model.collections.map(\.id)
                    ids.move(fromOffsets: from, toOffset: to)
                    Task { await model.moveCollections(ids) }
                }
                Button {
                    newCollection = false
                } label: {
                    Label(String(localized: "Neue Sammlung"), systemImage: "folder.badge.plus")
                }
                Button {
                    newCollection = true
                } label: {
                    Label(String(localized: "Neuer Filter-Feed"), systemImage: "line.3.horizontal.decrease")
                }
            } header: {
                Text(String(localized: "Sammlungen"))
            } footer: {
                Text(String(localized: "Eine Sammlung fasst Quellen zusammen. Mit Filterwörtern zeigt sie nur, was passt."))
            }

            Section {
                ForEach(model.sources, id: \.id) { source in
                    Button {
                        editingSource = source
                    } label: {
                        Row(
                            title: source.title,
                            subtitle: source.lastError.map { String(localized: "Fehler: \($0)") } ?? host(source.url),
                            badge: SourceBadge(source: source),
                            bell: source.notify,
                            warning: source.lastError != nil
                        )
                    }
                    .swipeActions {
                        Button(role: .destructive) {
                            Task { await model.deleteSource(source.id) }
                        } label: {
                            Label(String(localized: "Entfernen"), systemImage: "trash")
                        }
                        bellButton(on: source.notify) {
                            if !source.notify { _ = await Notifier.requestPermission() }
                            await model.updateSource(source.id, title: source.title, notify: !source.notify)
                        }
                    }
                }
                Button {
                    adding = true
                } label: {
                    Label(String(localized: "Quelle hinzufügen"), systemImage: "plus.circle")
                }
            } header: {
                Text(String(localized: "Quellen"))
            }
        }
        .listStyle(.insetGrouped)
        .canvasBackground()
        .navigationTitle(String(localized: "Feeds"))
        .toolbar {
            ToolbarItem(placement: .topBarLeading) { EditButton() }
            ToolbarItem(placement: .topBarTrailing) {
                Button { adding = true } label: { Image(systemName: "plus") }
                    .accessibilityLabel(String(localized: "Quelle hinzufügen"))
            }
        }
        .sheet(isPresented: $adding) { AddSourceSheet() }
        .sheet(item: $editingSource) { SourceEditor(source: $0) }
        .sheet(item: $editingCollection) { CollectionEditor(collection: $0) }
        .sheet(item: Binding(get: { newCollection.map(NewCollection.init) }, set: { newCollection = $0?.filter })) {
            CollectionEditor(collection: nil, startWithFilter: $0.filter)
        }
    }

    private func bellButton(on: Bool, _ action: @escaping @MainActor () async -> Void) -> some View {
        Button {
            Task { await action() }
        } label: {
            Label(
                on ? String(localized: "Stumm") : String(localized: "Benachrichtigen"),
                systemImage: on ? "bell.slash" : "bell"
            )
        }
        .tint(Palette.accent("violet"))
    }

    private func host(_ url: String) -> String? {
        URL(string: url)?.host()?.replacingOccurrences(of: "www.", with: "")
    }
}

private struct NewCollection: Identifiable {
    let filter: Bool
    var id: Bool { filter }
}

extension FeedSource: @retroactive Identifiable {}
extension FeedCollection: @retroactive Identifiable {}

/// Searching every article's title and summary.
struct SearchView: View {
    @Environment(AppModel.self) private var model
    @State private var text = ""
    @State private var results: [Article] = []

    var body: some View {
        List {
            ForEach(results, id: \.key) { article in
                NavigationLink {
                    ReaderView(key: article.key, sequence: results.map(\.key))
                } label: {
                    ArticleRow(article: article)
                }
                .contextMenu { ArticleMenu(article: article) }
            }
        }
        .listStyle(.plain)
        .canvasBackground()
        .navigationTitle(String(localized: "Suche"))
        .searchable(text: $text, prompt: String(localized: "Artikel durchsuchen"))
        .overlay {
            if !text.isEmpty, results.isEmpty {
                ContentUnavailableView.search(text: text)
            }
        }
        .task(id: "\(text)-\(model.revision)") {
            let query = text.trimmingCharacters(in: .whitespaces)
            guard query.count >= 2 else {
                results = []
                return
            }
            try? await Task.sleep(for: .milliseconds(200))
            guard !Task.isCancelled else { return }
            results = await model.articles(ArticleQuery(scope: .all, unreadOnly: false, search: query, limit: 100, before: nil, beforeKey: nil))
        }
    }
}
