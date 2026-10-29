import SwiftUI
import UwUNewsCore

/// Adding a source: paste a site or feed address, see what was found, add.
struct AddSourceSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var input = ""
    @State private var preview: FeedPreview?
    @State private var error: String?
    @State private var searching = false
    @State private var adding = false
    @State private var notify = true
    @State private var collectionIds: Set<String> = []
    @FocusState private var focused: Bool

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField(String(localized: "Webseite oder Feed-Adresse"), text: $input)
                        .keyboardType(.URL)
                        .textContentType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .submitLabel(.search)
                        .focused($focused)
                        .onSubmit { Task { await search() } }
                        .onChange(of: input) { preview = nil; error = nil }
                } footer: {
                    Text(String(localized: "Zum Beispiel die Startseite einer Nachrichtenseite oder eines Blogs. UwUNews sucht den Feed selbst."))
                }

                if searching {
                    Section {
                        HStack(spacing: 10) {
                            ProgressView().tint(Palette.pinkSolid)
                            Text(String(localized: "Suche nach einem Feed …")).foregroundStyle(Palette.muted)
                        }
                    }
                }

                if let error {
                    Section {
                        Label(error, systemImage: "exclamationmark.triangle.fill")
                            .foregroundStyle(Palette.danger)
                    }
                }

                if let preview {
                    Section(String(localized: "Gefunden")) {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(preview.title).font(UwUFont.style(.headline, weight: .bold))
                            Text(preview.url)
                                .font(UwUFont.style(.footnote))
                                .foregroundStyle(Palette.muted)
                                .lineLimit(2)
                            Text(String(localized: "\(Int(preview.itemCount)) Artikel im Feed"))
                                .font(UwUFont.style(.footnote))
                                .foregroundStyle(Palette.muted)
                        }
                        ForEach(preview.latest, id: \.self) { title in
                            Text(title).font(UwUFont.style(.subheadline)).lineLimit(2)
                        }
                    }
                    Section {
                        Toggle(String(localized: "Bei neuen Artikeln benachrichtigen"), isOn: $notify)
                    }
                    if !model.collections.isEmpty {
                        Section(String(localized: "In Sammlungen")) {
                            ForEach(model.collections.filter { !$0.allSources }, id: \.id) { collection in
                                Toggle(isOn: Binding(
                                    get: { collectionIds.contains(collection.id) },
                                    set: { on in
                                        if on { collectionIds.insert(collection.id) } else { collectionIds.remove(collection.id) }
                                    }
                                )) {
                                    Label {
                                        Text(collection.name)
                                    } icon: {
                                        SymbolBadge(symbol: collection.symbol, color: Palette.accent(collection.color), size: 26)
                                    }
                                }
                            }
                        }
                    }
                }
            }
            .canvasBackground()
            .navigationTitle(String(localized: "Quelle hinzufügen"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(String(localized: "Abbrechen")) { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    if preview == nil {
                        Button(String(localized: "Suchen")) { Task { await search() } }
                            .disabled(input.trimmingCharacters(in: .whitespaces).isEmpty || searching)
                    } else {
                        Button(String(localized: "Hinzufügen")) { Task { await add() } }
                            .buttonStyle(.glassProminent)
                            .disabled(adding)
                    }
                }
            }
            .onAppear {
                focused = true
                if input.isEmpty, let pasted = UIPasteboard.general.url ?? UIPasteboard.general.string.flatMap(URL.init(string:)),
                   pasted.scheme?.hasPrefix("http") == true {
                    input = pasted.absoluteString
                }
            }
        }
        .interactiveDismissDisabled(preview != nil)
    }

    private func search() async {
        searching = true
        error = nil
        defer { searching = false }
        switch await model.preview(input) {
        case let .success(found):
            preview = found
            focused = false
        case let .failure(failure):
            error = failure.readable
        }
    }

    private func add() async {
        guard let preview else { return }
        adding = true
        defer { adding = false }
        if notify { _ = await Notifier.requestPermission() }
        if await model.addSource(preview.url, notify: notify, collectionIds: Array(collectionIds)) != nil {
            dismiss()
        }
    }
}

/// A source's name, bell, collections; and removing it.
struct SourceEditor: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let source: FeedSource
    @State private var title: String
    @State private var notify: Bool
    @State private var collectionIds: Set<String>
    @State private var confirmDelete = false

    init(source: FeedSource) {
        self.source = source
        _title = State(initialValue: source.title)
        _notify = State(initialValue: source.notify)
        _collectionIds = State(initialValue: [])
    }

    var body: some View {
        NavigationStack {
            Form {
                Section(String(localized: "Name")) {
                    TextField(String(localized: "Name"), text: $title)
                }
                Section {
                    Toggle(String(localized: "Bei neuen Artikeln benachrichtigen"), isOn: $notify)
                } footer: {
                    Text(String(localized: "Sammlungen mit eigener Glocke melden ihre Artikel auch, wenn sie hier aus ist."))
                }
                let pickable = model.collections.filter { !$0.allSources }
                if !pickable.isEmpty {
                    Section(String(localized: "In Sammlungen")) {
                        ForEach(pickable, id: \.id) { collection in
                            Toggle(isOn: Binding(
                                get: { collectionIds.contains(collection.id) },
                                set: { on in
                                    if on { collectionIds.insert(collection.id) } else { collectionIds.remove(collection.id) }
                                }
                            )) {
                                Label {
                                    Text(collection.name)
                                } icon: {
                                    SymbolBadge(symbol: collection.symbol, color: Palette.accent(collection.color), size: 26)
                                }
                            }
                        }
                    }
                }
                Section(String(localized: "Feed")) {
                    LabeledContent(String(localized: "Adresse")) {
                        Text(source.url).textSelection(.enabled).lineLimit(3)
                    }
                    if let site = source.siteUrl, let url = URL(string: site) {
                        Link(destination: url) {
                            Label(String(localized: "Webseite öffnen"), systemImage: "safari")
                        }
                    }
                    if let checked = source.lastFetchedAt {
                        LabeledContent(String(localized: "Zuletzt abgerufen")) {
                            Text(Date(timeIntervalSince1970: TimeInterval(checked) / 1000), format: .relative(presentation: .named))
                        }
                    }
                    if let error = source.lastError {
                        Label(error, systemImage: "exclamationmark.triangle.fill").foregroundStyle(Palette.danger)
                    }
                }
                Section {
                    Button(String(localized: "Quelle entfernen"), role: .destructive) { confirmDelete = true }
                }
            }
            .canvasBackground()
            .navigationTitle(source.title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(String(localized: "Abbrechen")) { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(String(localized: "Sichern")) { Task { await save() } }
                        .buttonStyle(.glassProminent)
                        .disabled(title.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
            .confirmationDialog(
                String(localized: "„\(source.title)“ entfernen?"),
                isPresented: $confirmDelete,
                titleVisibility: .visible
            ) {
                Button(String(localized: "Entfernen"), role: .destructive) {
                    Task {
                        await model.deleteSource(source.id)
                        dismiss()
                    }
                }
            } message: {
                Text(String(localized: "Ihre Artikel verschwinden, gemerkte bleiben erhalten."))
            }
            .onAppear {
                collectionIds = Set(model.collections.filter { $0.sourceIds.contains(source.id) }.map(\.id))
            }
        }
    }

    private func save() async {
        if notify, !source.notify { _ = await Notifier.requestPermission() }
        await model.updateSource(source.id, title: title.trimmingCharacters(in: .whitespaces), notify: notify)
        for collection in model.collections where !collection.allSources {
            let inside = collection.sourceIds.contains(source.id)
            let wanted = collectionIds.contains(collection.id)
            guard inside != wanted else { continue }
            var changed = collection
            changed.sourceIds = wanted
                ? collection.sourceIds + [source.id]
                : collection.sourceIds.filter { $0 != source.id }
            await model.save(changed)
        }
        dismiss()
    }
}
