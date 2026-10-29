import SafariServices
import SwiftUI
import UwUNewsCore
import WebKit

/// One article as the feed carries it, in a web view with scripts off, and
/// the way to the original in the browser.
struct ReaderView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openURL) private var openURL
    @AppStorage("readerTextScale") private var textScale = 1.0
    @AppStorage("openLinksInApp") private var openLinksInApp = true

    let sequence: [String]
    @State private var key: String
    @State private var detail: ArticleDetail?
    @State private var browser: URL?

    init(key: String, sequence: [String]) {
        self.sequence = sequence
        _key = State(initialValue: key)
    }

    var body: some View {
        Group {
            if let detail {
                ArticleWebView(html: ReaderHTML.page(detail, scale: textScale)) { url in
                    open(url)
                }
                .ignoresSafeArea(edges: .bottom)
            } else {
                ProgressView().tint(Palette.pinkSolid)
            }
        }
        .background(Palette.canvas)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItemGroup(placement: .topBarTrailing) {
                if let article = detail?.article {
                    Button {
                        Task {
                            await model.setStarred(article.key, !article.starred)
                            await load()
                        }
                    } label: {
                        Image(systemName: article.starred ? "star.fill" : "star")
                    }
                    .accessibilityLabel(article.starred ? String(localized: "Nicht mehr merken") : String(localized: "Merken"))
                    if let link = article.link.flatMap(URL.init(string:)) {
                        ShareLink(item: link)
                    }
                }
                Menu {
                    Button { textScale = max(0.8, textScale - 0.1) } label: {
                        Label(String(localized: "Kleinere Schrift"), systemImage: "textformat.size.smaller")
                    }
                    Button { textScale = min(1.6, textScale + 0.1) } label: {
                        Label(String(localized: "Größere Schrift"), systemImage: "textformat.size.larger")
                    }
                    if let article = detail?.article {
                        Button {
                            Task {
                                await model.setRead([article.key], false)
                                await load()
                            }
                        } label: {
                            Label(String(localized: "Als ungelesen markieren"), systemImage: "circle.fill")
                        }
                    }
                } label: {
                    Image(systemName: "textformat")
                }
            }
            ToolbarItemGroup(placement: .bottomBar) {
                Button { step(-1) } label: { Image(systemName: "chevron.up") }
                    .disabled(index <= 0)
                    .accessibilityLabel(String(localized: "Vorheriger Artikel"))
                Button { step(1) } label: { Image(systemName: "chevron.down") }
                    .disabled(index < 0 || index >= sequence.count - 1)
                    .accessibilityLabel(String(localized: "Nächster Artikel"))
                Spacer()
                if let link = detail?.article.link.flatMap(URL.init(string:)), ["https", "http"].contains(link.scheme) {
                    Button {
                        openURL(link)
                    } label: {
                        Label(String(localized: "Im Browser öffnen"), systemImage: "safari")
                            .labelStyle(.titleAndIcon)
                    }
                    .buttonStyle(.glassProminent)
                }
            }
        }
        .sheet(item: Binding(get: { browser.map(BrowserPage.init) }, set: { browser = $0?.url })) { page in
            SafariView(url: page.url).ignoresSafeArea()
        }
        .task(id: key) {
            await load()
            if let article = detail?.article, !article.read {
                await model.setRead([article.key], true)
            }
        }
    }

    private var index: Int { sequence.firstIndex(of: key) ?? -1 }

    private func step(_ by: Int) {
        let next = index + by
        guard sequence.indices.contains(next) else { return }
        key = sequence[next]
    }

    private func load() async {
        detail = await model.detail(key)
    }

    private func open(_ url: URL) {
        guard url.scheme == "https" || url.scheme == "http" else {
            if url.scheme == "mailto" { openURL(url) }
            return
        }
        if openLinksInApp {
            browser = url
        } else {
            openURL(url)
        }
    }
}

private struct BrowserPage: Identifiable {
    let url: URL
    var id: String { url.absoluteString }
}

struct SafariView: UIViewControllerRepresentable {
    let url: URL

    func makeUIViewController(context _: Context) -> SFSafariViewController {
        let controller = SFSafariViewController(url: url)
        controller.preferredControlTintColor = UIColor(Palette.pinkSolid)
        return controller
    }

    func updateUIViewController(_: SFSafariViewController, context _: Context) {}
}

/// A web view for untrusted feed HTML: no scripts, no navigation inside it.
/// Taps on links go to `onLink`.
struct ArticleWebView: UIViewRepresentable {
    let html: String
    let onLink: (URL) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(onLink: onLink) }

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.defaultWebpagePreferences.allowsContentJavaScript = false
        config.websiteDataStore = .nonPersistent()
        config.dataDetectorTypes = []
        let view = WKWebView(frame: .zero, configuration: config)
        view.navigationDelegate = context.coordinator
        view.isOpaque = false
        view.backgroundColor = .clear
        view.scrollView.backgroundColor = .clear
        view.allowsLinkPreview = true
        return view
    }

    func updateUIView(_ view: WKWebView, context: Context) {
        context.coordinator.onLink = onLink
        if context.coordinator.loaded != html {
            context.coordinator.loaded = html
            view.loadHTMLString(html, baseURL: nil)
        }
    }

    final class Coordinator: NSObject, WKNavigationDelegate {
        var onLink: (URL) -> Void
        var loaded: String?

        init(onLink: @escaping (URL) -> Void) { self.onLink = onLink }

        @MainActor
        func webView(
            _: WKWebView,
            decidePolicyFor action: WKNavigationAction
        ) async -> WKNavigationActionPolicy {
            let mainFrame = action.targetFrame?.isMainFrame ?? true
            if action.navigationType == .linkActivated, let url = action.request.url {
                onLink(url)
                return .cancel
            }
            // The page itself (about:blank) and embedded players the CSP lets in; no redirects,
            // form posts or meta refreshes of the main frame.
            if action.navigationType == .other, !mainFrame || action.request.url?.scheme == "about" {
                return .allow
            }
            return .cancel
        }
    }
}

/// The reader's page: the article's header and its HTML in UwUSuite colours.
enum ReaderHTML {
    @MainActor private static let fontFace: String = {
        guard let url = Bundle.main.url(forResource: "UwUSans-wght", withExtension: "ttf"),
              let data = try? Data(contentsOf: url)
        else { return "" }
        return "@font-face{font-family:'UwU Sans';font-weight:200 800;src:url(data:font/ttf;base64,\(data.base64EncodedString()))format('truetype');}"
    }()

    @MainActor
    static func page(_ detail: ArticleDetail, scale: Double) -> String {
        let a = detail.article
        let date = Date(timeIntervalSince1970: TimeInterval(a.sortAt) / 1000)
            .formatted(date: .long, time: .shortened)
        let byline = [a.sourceTitle, a.author, date].compactMap { $0 }.map(escape).joined(separator: " · ")
        let hero: String = {
            guard let image = a.imageUrl, !detail.contentHtml.contains(image) else { return "" }
            return "<img class=\"hero\" src=\"\(escape(image))\" alt=\"\">"
        }()
        let title = a.link.map { "<a href=\"\(escape($0))\">\(escape(a.title))</a>" } ?? escape(a.title)
        let body = detail.contentHtml.isEmpty ? "<p>\(escape(a.summary))</p>" : detail.contentHtml
        return """
        <!doctype html><html><head><meta charset="utf-8">
        <meta name="viewport" content="width=device-width,initial-scale=1">
        <meta http-equiv="Content-Security-Policy" content="default-src 'none'; base-uri 'none'; form-action 'none'; img-src https: http: data:; media-src https:; style-src 'unsafe-inline'; font-src data:; frame-src https://www.youtube-nocookie.com https://player.vimeo.com">
        <style>\(fontFace)\(css(scale: scale))</style></head>
        <body><header><p class="byline">\(byline)</p><h1>\(title)</h1></header>\(hero)<article>\(body)</article></body></html>
        """
    }

    private static func css(scale: Double) -> String {
        """
        :root{color-scheme:light dark;--canvas:#f8f4f6;--ink:#1c1420;--muted:#716672;--hairline:#f2e8ee;--surface:#ffffff;--link:#e11d74;--tint:#ffe4ef}
        @media (prefers-color-scheme:dark){:root{--canvas:#141016;--ink:#f8f2f6;--muted:#b3a8b3;--hairline:#2c2430;--surface:#1c171f;--link:#ff7fac;--tint:#3a1a2a}}
        html{-webkit-text-size-adjust:none;font-size:\(Int(17 * scale))px}
        body{margin:0;padding:16px 20px 120px;background:var(--canvas);color:var(--ink);font:400 1rem/1.6 'UwU Sans',-apple-system,sans-serif;letter-spacing:-0.008em;word-wrap:break-word;overflow-wrap:anywhere}
        header{margin-bottom:18px}
        .byline{margin:0 0 6px;color:var(--muted);font-size:.8rem;font-weight:500}
        h1{margin:0;font-size:1.65rem;line-height:1.2;font-weight:760;letter-spacing:-0.02em}
        h1 a{color:inherit;text-decoration:none}
        h2,h3,h4{line-height:1.25;font-weight:700;margin:1.5em 0 .5em}
        a{color:var(--link);text-decoration-thickness:1px;text-underline-offset:2px}
        img,video,iframe,figure{max-width:100%;height:auto;border-radius:16px}
        img.hero{display:block;width:100%;margin:0 0 18px}
        figure{margin:1.2em 0}figcaption{color:var(--muted);font-size:.8rem;margin-top:.4em}
        blockquote{margin:1.2em 0;padding:.2em 0 .2em 1em;border-left:3px solid var(--link);color:var(--muted)}
        pre,code{font-family:ui-monospace,Menlo,monospace;font-size:.85em}
        pre{background:var(--surface);padding:12px 14px;border-radius:14px;overflow-x:auto;border:1px solid var(--hairline)}
        code{background:var(--tint);padding:.1em .3em;border-radius:6px}pre code{background:none;padding:0}
        table{border-collapse:collapse;display:block;overflow-x:auto}td,th{border:1px solid var(--hairline);padding:6px 8px}
        hr{border:0;border-top:1px solid var(--hairline);margin:2em 0}
        """
    }

    static func escape(_ text: String) -> String {
        text.replacingOccurrences(of: "&", with: "&amp;")
            .replacingOccurrences(of: "<", with: "&lt;")
            .replacingOccurrences(of: ">", with: "&gt;")
            .replacingOccurrences(of: "\"", with: "&quot;")
    }
}
