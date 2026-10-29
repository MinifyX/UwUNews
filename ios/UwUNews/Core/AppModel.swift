import Foundation
import Observation
import UIKit
import UwUNewsCore

/// The app's state: the Rust engine and what the screens show of it.
///
/// The engine blocks (SQLite and HTTP), so every call runs off the main actor
/// and the results come back here.
@MainActor
@Observable
final class AppModel {
    private(set) var sources: [FeedSource] = []
    private(set) var collections: [FeedCollection] = []
    private(set) var counts = Counts(unread: 0, starred: 0, today: 0)
    private(set) var refreshing = false
    private(set) var lastRefresh: Date?
    /// Bumped whenever articles may have changed, so lists reload.
    private(set) var revision = 0
    private(set) var sync: SyncStatus?
    private(set) var syncing = false
    var toast: Toast?
    private var pendingSync: Task<Void, Never>?
    /// Changes came in while a sync was running: go again after it.
    private var syncAgain = false

    let engine: Engine?
    let startupError: String?

    init() {
        switch Self.shared {
        case let .success(engine):
            self.engine = engine
            startupError = nil
        case let .failure(error):
            engine = nil
            startupError = error.localizedDescription
        }
    }

    /// One engine per process, for the screens and the background task alike.
    nonisolated static let shared: Result<Engine, Error> = Result {
        try Engine(dataDir: dataDirectory().path, deviceName: deviceName, secrets: KeychainSecrets())
    }

    /// What the other devices list this one as.
    nonisolated static let deviceName = "UwUNews · iPhone"

    nonisolated static func dataDirectory() throws -> URL {
        let dir = try FileManager.default.url(
            for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true
        ).appending(path: "UwUNews", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    // MARK: Reading

    func reload() async {
        guard let engine else { return }
        let result = await Self.off {
            (try engine.sources(), try engine.collections(), try engine.counts())
        }
        switch result {
        case let .success((sources, collections, counts)):
            self.sources = sources
            self.collections = collections
            self.counts = counts
        case let .failure(error):
            show(error)
        }
    }

    func articles(_ query: ArticleQuery) async -> [Article] {
        guard let engine else { return [] }
        switch await Self.off({ try engine.articles(query: query) }) {
        case let .success(list): return list
        case let .failure(error):
            show(error)
            return []
        }
    }

    func detail(_ key: String) async -> ArticleDetail? {
        guard let engine else { return nil }
        return try? await Self.off { try engine.article(key: key) }.get()
    }

    func source(_ id: String) -> FeedSource? { sources.first { $0.id == id } }
    func collection(_ id: String) -> FeedCollection? { collections.first { $0.id == id } }

    // MARK: Marks

    func setRead(_ keys: [String], _ read: Bool) async {
        await change { try $0.setRead(keys: keys, read: read) }
    }

    func setStarred(_ key: String, _ starred: Bool) async {
        await change { try $0.setStarred(key: key, starred: starred) }
    }

    func markAllRead(_ scope: Scope) async {
        await change { try $0.markAllRead(scope: scope) }
    }

    // MARK: Sources and collections

    func preview(_ input: String) async -> Result<FeedPreview, Error> {
        guard let engine else { return .failure(EngineMissing()) }
        return await Self.off { try engine.preview(input: input) }
    }

    @discardableResult
    func addSource(_ input: String, notify: Bool, collectionIds: [String]) async -> FeedSource? {
        guard let engine else { return nil }
        let result = await Self.off {
            try engine.addSource(input: input, notify: notify, collectionIds: collectionIds)
        }
        switch result {
        case let .success(source):
            await reload()
            revision += 1
            return source
        case let .failure(error):
            show(error)
            return nil
        }
    }

    func updateSource(_ id: String, title: String, notify: Bool) async {
        await change { try $0.updateSource(id: id, title: title, notify: notify) }
    }

    func deleteSource(_ id: String) async {
        await change { try $0.deleteSource(id: id) }
    }

    func save(_ collection: FeedCollection) async {
        await change { _ = try $0.saveCollection(collection: collection) }
    }

    func deleteCollection(_ id: String) async {
        await change { try $0.deleteCollection(id: id) }
    }

    func moveCollections(_ ids: [String]) async {
        await change { try $0.orderCollections(ids: ids) }
    }

    // MARK: Fetching

    /// Fetches every source (or one) and announces what is new.
    func refresh(source: String? = nil) async {
        guard let engine, !refreshing else { return }
        refreshing = true
        defer { refreshing = false }
        let report = await Self.offValue { engine.refresh(sourceId: source) }
        lastRefresh = .now
        await Notifier.announce(report.notices)
        let keep = UserDefaults.standard.object(forKey: "keepDays") as? Int ?? 60
        _ = await Self.off { try engine.prune(keepDays: UInt32(keep)) }
        await reload()
        revision += 1
        if source == nil { await syncNow(quiet: true) }
        if report.failed > 0, source != nil, let first = report.errors.first {
            toast = Toast(title: String(localized: "Abruf fehlgeschlagen"), detail: first, tone: .danger)
        }
    }

    // MARK: Sync

    func loadSync() async {
        guard let engine else { return }
        sync = try? await Self.off { try engine.syncStatus() }.get()
    }

    var syncConnected: Bool {
        guard let sync else { return false }
        if case .off = sync.backend { return false }
        return true
    }

    /// One pass up and down. `quiet` keeps failures out of sight: the sync
    /// screen shows them.
    func syncNow(quiet: Bool = false) async {
        guard let engine else { return }
        guard !syncing else {
            syncAgain = true
            return
        }
        syncing = true
        defer {
            syncing = false
            if syncAgain {
                syncAgain = false
                syncSoon()
            }
        }
        await loadSync()
        guard syncConnected else { return }
        let result = await Self.off { try engine.syncNow() }
        switch result {
        case let .success(summary):
            if summary.pulled > 0 {
                await reload()
                revision += 1
            }
            lastSyncError = nil
        case let .failure(error):
            lastSyncError = error.readable
            if !quiet { show(error) }
        }
        await loadSync()
    }

    private(set) var lastSyncError: String?

    /// A step of setting sync up, off the main thread; the status follows.
    func syncAction<T: Sendable>(_ work: @escaping @Sendable (Engine) throws -> T) async -> Result<T, Error> {
        guard let engine else { return .failure(EngineMissing()) }
        let result = await Self.off { try work(engine) }
        await loadSync()
        return result
    }

    /// A sync shortly after local changes, so a burst of them goes up once.
    private func syncSoon() {
        guard syncConnected else { return }
        pendingSync?.cancel()
        pendingSync = Task {
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled else { return }
            await syncNow(quiet: true)
        }
    }

    // MARK: OPML

    func importOpml(_ data: Data) async {
        guard let engine else { return }
        switch await Self.off({ try engine.importOpml(data: data) }) {
        case let .success(count):
            await reload()
            toast = Toast(
                title: String(localized: "\(Int(count)) Quellen importiert"),
                detail: String(localized: "Neue Artikel werden jetzt geladen"),
                tone: .success
            )
            await refresh()
        case let .failure(error):
            show(error)
        }
    }

    func exportOpml() async -> String? {
        guard let engine else { return nil }
        return try? await Self.off { try engine.exportOpml() }.get()
    }

    // MARK: Plumbing

    private func change(_ work: @escaping @Sendable (Engine) throws -> Void) async {
        guard let engine else { return }
        if case let .failure(error) = await Self.off({ try work(engine) }) {
            show(error)
        }
        await reload()
        revision += 1
        syncSoon()
    }

    func show(_ error: Error) {
        toast = Toast(title: String(localized: "Das hat nicht geklappt"), detail: error.readable, tone: .danger)
    }

    /// Engine calls block on SQLite and the network, some for minutes (a
    /// pairing waits for the other device). They run on their own threads so
    /// Swift's small cooperative pool stays free.
    private nonisolated static let blocking = DispatchQueue(label: "app.uwunews.engine", qos: .userInitiated, attributes: .concurrent)

    nonisolated static func off<T: Sendable>(_ work: @escaping @Sendable () throws -> T) async -> Result<T, Error> {
        await withCheckedContinuation { continuation in
            blocking.async { continuation.resume(returning: Result { try work() }) }
        }
    }

    nonisolated static func offValue<T: Sendable>(_ work: @escaping @Sendable () -> T) async -> T {
        await withCheckedContinuation { continuation in
            blocking.async { continuation.resume(returning: work()) }
        }
    }
}

struct EngineMissing: LocalizedError {
    var errorDescription: String? { String(localized: "Der Speicher der App ließ sich nicht öffnen.") }
}

struct Toast: Identifiable, Equatable {
    enum Tone { case info, success, danger }
    let id = UUID()
    var title: String
    var detail: String?
    var tone: Tone = .info
}

extension Error {
    /// What the engine says, without UniFFI's type name around it.
    var readable: String {
        if let error = self as? NewsError {
            switch error {
            case let .Network(message): return String(localized: "Keine Verbindung: \(message)")
            case let .NotAFeed(message): return String(localized: "Unter dieser Adresse ist kein Feed. \(message)")
            case let .Invalid(message): return message
            case let .Storage(message): return String(localized: "Speicherfehler: \(message)")
            case let .Sync(message): return message
            }
        }
        return localizedDescription
    }
}

extension Scope {
    var isStarred: Bool {
        if case .starred = self { return true }
        return false
    }
}
