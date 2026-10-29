import SwiftUI
import UwUNewsCore

/// A collection: a folder of sources, and with words a filter feed.
struct CollectionEditor: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let original: FeedCollection?
    @State private var draft: FeedCollection
    @State private var showFilter: Bool
    @State private var confirmDelete = false

    static let symbols = [
        "folder", "newspaper", "globe.europe.africa", "laptopcomputer", "cpu", "gamecontroller",
        "film", "music.note", "book", "graduationcap", "flask", "leaf", "sportscourt", "car",
        "airplane", "fork.knife", "heart", "star", "bolt", "flame", "building.columns",
        "chart.line.uptrend.xyaxis", "camera", "paintpalette", "hammer", "pawprint",
    ]

    init(collection: FeedCollection?, startWithFilter: Bool = false) {
        original = collection
        let draft = collection ?? FeedCollection(
            id: "",
            name: "",
            symbol: startWithFilter ? "line.3.horizontal.decrease" : "folder",
            color: "pink",
            allSources: startWithFilter,
            sourceIds: [],
            any: [],
            all: [],
            exclude: [],
            searchContent: false,
            notify: false,
            unread: 0
        )
        _draft = State(initialValue: draft)
        _showFilter = State(initialValue: startWithFilter || !(draft.any.isEmpty && draft.all.isEmpty && draft.exclude.isEmpty))
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    HStack(spacing: 12) {
                        SymbolBadge(symbol: draft.symbol, color: Palette.accent(draft.color), size: 40)
                        TextField(String(localized: "Name"), text: $draft.name)
                            .font(UwUFont.style(.title3, weight: .semibold))
                    }
                    colorPicker
                    symbolPicker
                }

                Section {
                    Toggle(String(localized: "Alle Quellen"), isOn: $draft.allSources)
                    if !draft.allSources {
                        ForEach(model.sources, id: \.id) { source in
                            Button {
                                toggle(source.id)
                            } label: {
                                HStack {
                                    SourceBadge(source: source, size: 26)
                                    Text(source.title).foregroundStyle(Palette.ink)
                                    Spacer()
                                    if draft.sourceIds.contains(source.id) {
                                        Image(systemName: "checkmark").foregroundStyle(Palette.pinkSolid).fontWeight(.semibold)
                                    }
                                }
                            }
                        }
                    }
                } header: {
                    Text(String(localized: "Quellen"))
                }

                Section {
                    Toggle(String(localized: "Nach Wörtern filtern"), isOn: $showFilter.animation())
                } footer: {
                    if !showFilter {
                        Text(String(localized: "Ohne Filter zeigt die Sammlung alles aus ihren Quellen."))
                    }
                }
                if showFilter {
                    WordList(
                        title: String(localized: "Mindestens eins davon"),
                        footer: String(localized: "Leer lassen, wenn es egal ist."),
                        words: $draft.any
                    )
                    WordList(
                        title: String(localized: "Alle davon"),
                        footer: nil,
                        words: $draft.all
                    )
                    WordList(
                        title: String(localized: "Keins davon"),
                        footer: String(localized: "Wörter passen auch als Anfang: „klima“ findet „Klimawandel“. Groß- und Kleinschreibung und Akzente zählen nicht."),
                        words: $draft.exclude
                    )
                    Section {
                        Picker(String(localized: "Suchen in"), selection: $draft.searchContent) {
                            Text(String(localized: "Titel")).tag(false)
                            Text(String(localized: "Titel und Text")).tag(true)
                        }
                        .pickerStyle(.segmented)
                    } header: {
                        Text(String(localized: "Suchen in"))
                    }
                }

                Section {
                    Toggle(String(localized: "Bei neuen Artikeln benachrichtigen"), isOn: $draft.notify)
                } footer: {
                    Text(String(localized: "Meldet jeden neuen Artikel, der in diese Sammlung fällt, auch aus Quellen ohne Glocke."))
                }

                if original != nil {
                    Section {
                        Button(String(localized: "Sammlung löschen"), role: .destructive) { confirmDelete = true }
                    } footer: {
                        Text(String(localized: "Die Quellen und ihre Artikel bleiben."))
                    }
                }
            }
            .canvasBackground()
            .navigationTitle(original == nil ? String(localized: "Neue Sammlung") : draft.name)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(String(localized: "Abbrechen")) { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(original == nil ? String(localized: "Anlegen") : String(localized: "Sichern")) {
                        Task { await save() }
                    }
                    .buttonStyle(.glassProminent)
                    .disabled(draft.name.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
            .confirmationDialog(
                String(localized: "„\(draft.name)“ löschen?"),
                isPresented: $confirmDelete,
                titleVisibility: .visible
            ) {
                Button(String(localized: "Löschen"), role: .destructive) {
                    Task {
                        await model.deleteCollection(draft.id)
                        dismiss()
                    }
                }
            }
        }
        .interactiveDismissDisabled()
    }

    private var colorPicker: some View {
        HStack {
            ForEach(Palette.accents, id: \.name) { accent in
                Button {
                    draft.color = accent.name
                } label: {
                    Circle()
                        .fill(accent.color.gradient)
                        .frame(width: 30, height: 30)
                        .overlay {
                            if draft.color == accent.name {
                                Circle().strokeBorder(Palette.ink.opacity(0.7), lineWidth: 2.5).padding(-4)
                            }
                        }
                }
                .buttonStyle(.plain)
                .frame(maxWidth: .infinity)
                .accessibilityLabel(accent.name)
            }
        }
        .padding(.vertical, 6)
    }

    private var symbolPicker: some View {
        LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 8), count: 7), spacing: 10) {
            ForEach(Self.symbols + ["line.3.horizontal.decrease"], id: \.self) { symbol in
                Button {
                    draft.symbol = symbol
                } label: {
                    Image(systemName: symbol)
                        .font(.system(size: 16, weight: .medium))
                        .frame(width: 38, height: 38)
                        .foregroundStyle(draft.symbol == symbol ? Palette.pinkInk : Palette.muted)
                        .background(
                            draft.symbol == symbol ? Palette.pinkTint : Palette.hairline,
                            in: Circle()
                        )
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.vertical, 6)
    }

    private func toggle(_ id: String) {
        if let at = draft.sourceIds.firstIndex(of: id) {
            draft.sourceIds.remove(at: at)
        } else {
            draft.sourceIds.append(id)
        }
    }

    private func save() async {
        var collection = draft
        collection.name = collection.name.trimmingCharacters(in: .whitespaces)
        if !showFilter {
            collection.any = []
            collection.all = []
            collection.exclude = []
        }
        if collection.notify { _ = await Notifier.requestPermission() }
        await model.save(collection)
        dismiss()
    }
}

/// A section of words with a field to add one and swipe to remove.
private struct WordList: View {
    let title: String
    let footer: String?
    @Binding var words: [String]
    @State private var input = ""

    var body: some View {
        Section {
            ForEach(words, id: \.self) { word in
                Text(word)
            }
            .onDelete { words.remove(atOffsets: $0) }
            HStack {
                TextField(String(localized: "Wort oder Wortgruppe"), text: $input)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.done)
                    .onSubmit(add)
                Button(action: add) {
                    Image(systemName: "plus.circle.fill")
                }
                .disabled(input.trimmingCharacters(in: .whitespaces).isEmpty)
                .accessibilityLabel(String(localized: "Hinzufügen"))
            }
        } header: {
            Text(title)
        } footer: {
            if let footer { Text(footer) }
        }
    }

    private func add() {
        for part in input.split(separator: ",") {
            let word = part.trimmingCharacters(in: .whitespaces)
            if !word.isEmpty, !words.contains(where: { $0.caseInsensitiveCompare(word) == .orderedSame }) {
                words.append(word)
            }
        }
        input = ""
    }
}
