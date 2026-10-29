import SwiftUI
import UwUNewsCore

/// Connecting to UwUSync or UwULock, and the state once connected.
struct SyncSettingsView: View {
    @Environment(AppModel.self) private var model
    @State private var service: Service = .uwuSync
    @State private var confirmStop = false

    enum Service: Hashable { case uwuSync, uwuLock }

    var body: some View {
        List {
            if let sync = model.sync {
                switch sync.backend {
                case .off:
                    Section {
                        Picker(String(localized: "Dienst"), selection: $service) {
                            Text("UwUSync").tag(Service.uwuSync)
                            Text("UwULock").tag(Service.uwuLock)
                        }
                        .pickerStyle(.segmented)
                    } footer: {
                        Text(service == .uwuSync
                            ? String(localized: "Ein eigenes UwUSync-Konto, wie bei UwUSSH und UwURDP. Der Server sieht nur verschlüsselte Daten.")
                            : String(localized: "Mit deinem UwULock-Konto: UwUNews bekommt dort einen eigenen Bereich."))
                    }
                    if service == .uwuSync {
                        UwuSyncSetup()
                    } else {
                        LockSignInForm()
                    }
                case let .uwuSync(serverUrl, accountId):
                    StatusSection(sync: sync, server: serverUrl, account: accountId)
                    VaultSection(sync: sync)
                    PairingSection()
                    stopSection
                case let .uwuLock(serverUrl, email, signedIn):
                    StatusSection(sync: sync, server: serverUrl, account: email)
                    if !signedIn {
                        LockSignInForm(serverUrl: serverUrl, email: email)
                    }
                    VaultSection(sync: sync)
                    stopSection
                }
            } else {
                ProgressView()
                    .frame(maxWidth: .infinity)
                    .listRowBackground(Color.clear)
            }
        }
        .listStyle(.insetGrouped)
        .canvasBackground()
        .navigationTitle(String(localized: "Sync"))
        .task { await model.loadSync() }
        .confirmationDialog(String(localized: "Sync beenden?"), isPresented: $confirmStop, titleVisibility: .visible) {
            Button(String(localized: "Sync beenden"), role: .destructive) {
                Task {
                    if case let .failure(error) = await model.syncAction({ try $0.disconnectSync() }) {
                        model.show(error)
                    }
                }
            }
        } message: {
            Text(String(localized: "Alles auf diesem iPhone bleibt, es wird nur nicht mehr abgeglichen."))
        }
    }

    private var stopSection: some View {
        Section {
            Button(String(localized: "Sync beenden"), role: .destructive) { confirmStop = true }
        }
    }
}

// MARK: Connected

private struct StatusSection: View {
    @Environment(AppModel.self) private var model
    let sync: SyncStatus
    let server: String
    let account: String

    var body: some View {
        Section {
            LabeledContent(String(localized: "Server"), value: server)
            LabeledContent(String(localized: "Konto"), value: account)
            LabeledContent(String(localized: "Ausstehend"), value: "\(sync.pending)")
            LabeledContent(String(localized: "Zuletzt abgeglichen")) {
                if let at = sync.lastSyncAt {
                    Text(Date(timeIntervalSince1970: Double(at) / 1000), format: .relative(presentation: .named))
                } else {
                    Text(String(localized: "Noch nie"))
                }
            }
            Button {
                Task { await model.syncNow() }
            } label: {
                HStack {
                    Label(String(localized: "Jetzt abgleichen"), systemImage: "arrow.triangle.2.circlepath")
                    Spacer()
                    if model.syncing { ProgressView() }
                }
            }
            .disabled(model.syncing || sync.vault != .unlocked)
        } header: {
            Text(String(localized: "Verbunden"))
        } footer: {
            if let error = model.lastSyncError {
                Text(error).foregroundStyle(Palette.danger)
            }
        }
    }
}

/// The master password, when this device lost the vault key.
private struct VaultSection: View {
    @Environment(AppModel.self) private var model
    let sync: SyncStatus
    @State private var password = ""
    @State private var busy = false

    var body: some View {
        if sync.vault == .locked {
            Section {
                SecureField(String(localized: "Master-Passwort"), text: $password)
                    .textContentType(.password)
                Button(String(localized: "Entsperren")) {
                    busy = true
                    Task {
                        let password = password
                        if case let .failure(error) = await model.syncAction({ try $0.unlockSync(password: password) }) {
                            model.show(error)
                        } else {
                            self.password = ""
                            await model.syncNow()
                        }
                        busy = false
                    }
                }
                .disabled(password.isEmpty || busy)
            } header: {
                Text(String(localized: "Gesperrt"))
            } footer: {
                Text(String(localized: "Der Schlüssel fehlt auf diesem Gerät. Mit dem Master-Passwort kommt er zurück."))
            }
        }
    }
}

/// Lets another device join: it types the words or pastes the code.
private struct PairingSection: View {
    @Environment(AppModel.self) private var model
    @State private var code: PairingCode?
    @State private var waiting: Task<Void, Never>?

    var body: some View {
        Section {
            if let code {
                VStack(alignment: .leading, spacing: 8) {
                    Text(code.spoken)
                        .font(UwUFont.style(.title3, weight: .semibold))
                        .textSelection(.enabled)
                    HStack {
                        ProgressView()
                        Text(String(localized: "Warte auf das andere Gerät …"))
                            .foregroundStyle(Palette.muted)
                    }
                }
                Button {
                    UIPasteboard.general.string = code.pasteable
                    model.toast = Toast(title: String(localized: "Code kopiert"), tone: .success)
                } label: {
                    Label(String(localized: "Code zum Einfügen kopieren"), systemImage: "doc.on.doc")
                }
                Button(String(localized: "Abbrechen"), role: .cancel) { stop() }
            } else {
                Button {
                    offer()
                } label: {
                    Label(String(localized: "Weiteres Gerät hinzufügen"), systemImage: "plus.circle")
                }
            }
        } header: {
            Text(String(localized: "Geräte"))
        } footer: {
            Text(String(localized: "Auf dem anderen Gerät „Mit Konto verbinden“ wählen und die Wörter eintippen oder den Code einfügen. Der Code gilt fünf Minuten."))
        }
        .onDisappear { stop() }
    }

    private func offer() {
        waiting = Task {
            let offered = await model.syncAction { try $0.offerPairing() }
            guard case let .success(offered) = offered else {
                if case let .failure(error) = offered { model.show(error) }
                return
            }
            guard !Task.isCancelled else {
                _ = await model.syncAction { try $0.cancelPairing() }
                return
            }
            code = offered
            let joined = await model.syncAction { try $0.waitForDevice(timeoutSecs: 300) }
            guard !Task.isCancelled else { return }
            code = nil
            switch joined {
            case let .success(name):
                model.toast = Toast(title: String(localized: "\(name) ist verbunden"), tone: .success)
            case let .failure(error):
                model.show(error)
            }
        }
    }

    /// Withdraws the code on the server too, so nobody can still use it.
    private func stop() {
        guard let waiting else { return }
        waiting.cancel()
        self.waiting = nil
        code = nil
        Task { _ = await model.syncAction { try $0.cancelPairing() } }
    }
}

// MARK: Setting up

private struct UwuSyncSetup: View {
    @Environment(AppModel.self) private var model
    @State private var creating = false
    @State private var code = ""
    @State private var serverUrl = ""
    @State private var fingerprint = ""
    @State private var password = ""
    @State private var repeated = ""
    @State private var flavor: AccountFlavor = .ssh
    @State private var busy = false
    @State private var recovery: String?

    var body: some View {
        if let recovery {
            Section {
                Text(recovery)
                    .font(.system(.body, design: .monospaced))
                    .textSelection(.enabled)
                Button {
                    UIPasteboard.general.string = recovery
                } label: {
                    Label(String(localized: "Kopieren"), systemImage: "doc.on.doc")
                }
                Button(String(localized: "Aufgeschrieben")) {
                    self.recovery = nil
                    Task { await model.syncNow() }
                }
                .buttonStyle(.glassProminent)
            } header: {
                Text(String(localized: "Wiederherstellungscode"))
            } footer: {
                Text(String(localized: "Schreib ihn auf und bewahr ihn sicher auf. Ohne ihn und ohne Master-Passwort sind die Daten verloren."))
            }
        } else {
            Section {
                Picker(String(localized: "Konto"), selection: $creating) {
                    Text(String(localized: "Vorhandenes")).tag(false)
                    Text(String(localized: "Neues")).tag(true)
                }
                .pickerStyle(.segmented)
                if creating {
                    TextField(String(localized: "Einrichtungscode vom Server"), text: $code, axis: .vertical)
                        .monospaced()
                } else {
                    TextField(String(localized: "Code vom anderen Gerät"), text: $code, axis: .vertical)
                        .monospaced()
                    Picker(String(localized: "Konto von"), selection: $flavor) {
                        Text("UwUNews / UwUSSH").tag(AccountFlavor.ssh)
                        Text("UwURDP").tag(AccountFlavor.rdp)
                    }
                }
                SecureField(String(localized: "Master-Passwort"), text: $password)
                    .textContentType(creating ? .newPassword : .password)
                if creating {
                    SecureField(String(localized: "Master-Passwort wiederholen"), text: $repeated)
                        .textContentType(.newPassword)
                }
            } footer: {
                Text(creating
                    ? String(localized: "Den Einrichtungscode zeigt der UwUSync-Server beim ersten Start oder per Admin-Befehl.")
                    : String(localized: "Auf einem Gerät, das schon verbunden ist: „Weiteres Gerät hinzufügen“."))
            }
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()

            if !creating {
                Section {
                    TextField(String(localized: "Server-Adresse"), text: $serverUrl)
                        .keyboardType(.URL)
                    TextField(String(localized: "Fingerabdruck (optional)"), text: $fingerprint)
                        .monospaced()
                } header: {
                    Text(String(localized: "Nur bei gesprochenem Code"))
                } footer: {
                    Text(String(localized: "Der eingefügte Code enthält den Server schon."))
                }
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            }

            Section {
                Button {
                    submit()
                } label: {
                    HStack {
                        Text(creating ? String(localized: "Konto anlegen") : String(localized: "Verbinden"))
                        Spacer()
                        if busy { ProgressView() }
                    }
                }
                .disabled(!ready || busy)
            }
        }
    }

    private var ready: Bool {
        let filled = !code.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !password.isEmpty
        return creating ? filled && password == repeated : filled
    }

    private func submit() {
        busy = true
        let (code, password, server, fingerprint, flavor) = (code, password, serverUrl, fingerprint, flavor)
        Task {
            if creating {
                switch await model.syncAction({ try $0.createSyncAccount(setupCode: code, password: password) }) {
                case let .success(recovery): self.recovery = recovery
                case let .failure(error): model.show(error)
                }
            } else {
                let result = await model.syncAction {
                    try $0.joinSync(code: code, serverUrl: server, fingerprint: fingerprint, password: password, flavor: flavor)
                }
                switch result {
                case .success:
                    model.toast = Toast(title: String(localized: "Verbunden"), tone: .success)
                    await model.syncNow()
                case let .failure(error):
                    model.show(error)
                }
            }
            busy = false
        }
    }
}

private struct LockSignInForm: View {
    @Environment(AppModel.self) private var model
    @State var serverUrl = ""
    @State var email = ""
    @State private var password = ""
    @State private var options: [TwoFactorOption] = []
    @State private var provider: UInt8?
    @State private var twoFactorCode = ""
    @State private var remember = true
    @State private var message: String?
    @State private var space: (was: String, now: String?)?
    @State private var busy = false

    var body: some View {
        Section {
            TextField(String(localized: "Server-Adresse"), text: $serverUrl)
                .keyboardType(.URL)
                .textContentType(.URL)
            TextField(String(localized: "E-Mail"), text: $email)
                .keyboardType(.emailAddress)
                .textContentType(.username)
            SecureField(String(localized: "Master-Passwort"), text: $password)
                .textContentType(.password)
        } header: {
            Text(String(localized: "Anmelden"))
        }
        .textInputAutocapitalization(.never)
        .autocorrectionDisabled()

        if !options.isEmpty {
            Section {
                Picker(String(localized: "Methode"), selection: $provider) {
                    ForEach(options.filter(\.supported), id: \.provider) { option in
                        Text(Self.name(option.kind)).tag(Optional(option.provider))
                    }
                }
                TextField(String(localized: "Code"), text: $twoFactorCode)
                    .keyboardType(.numberPad)
                    .textContentType(.oneTimeCode)
                if let chosen, chosen.kind == "email" {
                    Button(String(localized: "Code per E-Mail senden")) { sendEmail() }
                        .disabled(busy)
                }
                Toggle(String(localized: "Dieses Gerät merken"), isOn: $remember)
            } header: {
                Text(String(localized: "Zweiter Faktor"))
            } footer: {
                if let message { Text(message) } else if let hint = chosen?.hint { Text(hint) }
            }
        }

        Section {
            Button {
                signIn()
            } label: {
                HStack {
                    Text(String(localized: "Anmelden"))
                    Spacer()
                    if busy { ProgressView() }
                }
            }
            .disabled(serverUrl.isEmpty || email.isEmpty || password.isEmpty || busy)
        }
        .confirmationDialog(
            String(localized: "Anderer Bereich"),
            isPresented: Binding(get: { space != nil }, set: { if !$0 { space = nil } }),
            titleVisibility: .visible
        ) {
            Button(String(localized: "Diesen Bereich nehmen")) {
                let accept = space?.now ?? "new"
                space = nil
                signIn(accept: accept)
            }
        } message: {
            Text(String(localized: "Das Konto hat einen anderen UwUNews-Bereich als bisher. Auf dieses iPhone kommt dann, was dort liegt; was hier ist, geht mit hoch."))
        }
    }

    private var chosen: TwoFactorOption? {
        options.first { $0.provider == provider }
    }

    private static func name(_ kind: String) -> String {
        switch kind {
        case "authenticator": String(localized: "Authenticator-App")
        case "email": String(localized: "E-Mail")
        case "yubikey": "YubiKey"
        case "duo": "Duo"
        case "webauthn": String(localized: "Passkey")
        default: kind
        }
    }

    private func signIn(accept: String? = nil) {
        busy = true
        let sign = LockSignIn(
            serverUrl: serverUrl,
            email: email,
            password: password,
            twoFactorProvider: options.isEmpty ? nil : provider,
            twoFactorCode: options.isEmpty ? nil : twoFactorCode,
            remember: remember,
            acceptSpace: accept
        )
        Task {
            switch await model.syncAction({ try $0.connectLock(signIn: sign) }) {
            case .success(.signedIn):
                password = ""
                options = []
                model.toast = Toast(title: String(localized: "Angemeldet"), tone: .success)
                await model.syncNow()
            case let .success(.twoFactor(offered, text)):
                options = offered
                message = text
                if chosen == nil { provider = offered.first(where: \.supported)?.provider }
                if offered.allSatisfy({ !$0.supported }) {
                    model.toast = Toast(
                        title: String(localized: "Kein passender zweiter Faktor"),
                        detail: String(localized: "Richte in UwULock eine Authenticator-App oder E-Mail-Codes ein."),
                        tone: .danger
                    )
                }
            case let .success(.spaceChanged(was, now)):
                space = (was, now)
            case let .failure(error):
                model.show(error)
            }
            busy = false
        }
    }

    private func sendEmail() {
        busy = true
        let (server, email, password) = (serverUrl, email, password)
        Task {
            if case let .failure(error) = await model.syncAction({
                try $0.sendLockEmailCode(serverUrl: server, email: email, password: password)
            }) {
                model.show(error)
            } else {
                model.toast = Toast(title: String(localized: "Code ist unterwegs"), tone: .info)
            }
            busy = false
        }
    }
}
