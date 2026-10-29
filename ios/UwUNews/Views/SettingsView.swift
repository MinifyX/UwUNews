import SwiftUI
import UniformTypeIdentifiers
import UserNotifications
import UwUNewsCore

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openURL) private var openURL
    @AppStorage("openLinksInApp") private var openLinksInApp = true
    @AppStorage("keepDays") private var keepDays = 60
    @State private var permission: UNAuthorizationStatus = .notDetermined
    @State private var importing = false
    @State private var exported: OpmlFile?

    var body: some View {
        List {
            Section {
                NavigationLink {
                    SyncSettingsView()
                } label: {
                    Label(String(localized: "Sync"), systemImage: "arrow.triangle.2.circlepath")
                }
            } footer: {
                Text(String(localized: "Quellen, Sammlungen und was gelesen oder gemerkt ist, Ende-zu-Ende-verschlüsselt über UwUSync oder UwULock."))
            }

            Section {
                switch permission {
                case .authorized, .provisional, .ephemeral:
                    Label(String(localized: "Mitteilungen erlaubt"), systemImage: "bell.badge")
                case .denied:
                    Button {
                        if let url = URL(string: UIApplication.openNotificationSettingsURLString) { openURL(url) }
                    } label: {
                        Label(String(localized: "Mitteilungen in den Einstellungen erlauben"), systemImage: "bell.slash")
                    }
                default:
                    Button {
                        Task {
                            _ = await Notifier.requestPermission()
                            await check()
                        }
                    } label: {
                        Label(String(localized: "Mitteilungen erlauben"), systemImage: "bell")
                    }
                }
            } header: {
                Text(String(localized: "Mitteilungen"))
            } footer: {
                Text(String(localized: "Welche Quellen und Sammlungen melden, stellst du an ihrer Glocke ein. iOS entscheidet, wie oft UwUNews im Hintergrund nachsieht – meist alle ein bis zwei Stunden, öfter, wenn du die App viel nutzt."))
            }

            Section(String(localized: "Lesen")) {
                Toggle(String(localized: "Links in der App öffnen"), isOn: $openLinksInApp)
                Picker(String(localized: "Gelesene Artikel behalten"), selection: $keepDays) {
                    Text(String(localized: "2 Wochen")).tag(14)
                    Text(String(localized: "1 Monat")).tag(30)
                    Text(String(localized: "2 Monate")).tag(60)
                    Text(String(localized: "6 Monate")).tag(180)
                }
            }

            Section {
                Button {
                    importing = true
                } label: {
                    Label(String(localized: "OPML importieren"), systemImage: "square.and.arrow.down")
                }
                Button {
                    Task {
                        if let text = await model.exportOpml() { exported = OpmlFile(text: text) }
                    }
                } label: {
                    Label(String(localized: "OPML exportieren"), systemImage: "square.and.arrow.up")
                }
                .disabled(model.sources.isEmpty)
            } header: {
                Text(String(localized: "Umziehen"))
            } footer: {
                Text(String(localized: "OPML ist die Liste, die fast jeder Feed-Reader lesen und schreiben kann."))
            }

            Section(String(localized: "Über")) {
                LabeledContent(String(localized: "Version"), value: Self.version)
                Link(destination: URL(string: "https://github.com/MinifyX/UwUNews")!) {
                    Label(String(localized: "Quellcode (GPL-3.0)"), systemImage: "chevron.left.forwardslash.chevron.right")
                }
            }
        }
        .listStyle(.insetGrouped)
        .canvasBackground()
        .navigationTitle(String(localized: "Einstellungen"))
        .task { await check() }
        .fileImporter(isPresented: $importing, allowedContentTypes: OpmlFile.readable) { result in
            guard case let .success(url) = result else { return }
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            guard let data = try? Data(contentsOf: url) else { return }
            Task { await model.importOpml(data) }
        }
        .fileExporter(
            isPresented: Binding(get: { exported != nil }, set: { if !$0 { exported = nil } }),
            document: exported,
            contentType: OpmlFile.type,
            defaultFilename: "UwUNews.opml"
        ) { _ in exported = nil }
    }

    private func check() async {
        permission = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
    }

    static var version: String {
        let info = Bundle.main.infoDictionary
        let short = info?["CFBundleShortVersionString"] as? String ?? "?"
        let build = info?["CFBundleVersion"] as? String ?? "?"
        return "\(short) (\(build))"
    }
}

struct OpmlFile: FileDocument {
    static let type = UTType(filenameExtension: "opml", conformingTo: .xml) ?? .xml
    static let readable: [UTType] = [type, .xml, .plainText, .data]
    static var readableContentTypes: [UTType] { readable }

    var text: String

    init(text: String) { self.text = text }

    init(configuration: ReadConfiguration) throws {
        text = String(decoding: configuration.file.regularFileContents ?? Data(), as: UTF8.self)
    }

    func fileWrapper(configuration _: WriteConfiguration) throws -> FileWrapper {
        FileWrapper(regularFileWithContents: Data(text.utf8))
    }
}
