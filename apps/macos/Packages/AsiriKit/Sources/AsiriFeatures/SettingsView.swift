import AppKit
import AsiriDesign
import AsiriDomain
import AsiriPlatform
import SwiftUI

/// Settings (§13.3): profile, privacy, security, network, server and about.
public struct SettingsView: View {
    let app: AppModel

    public init(app: AppModel) {
        self.app = app
    }

    public var body: some View {
        TabView {
            ProfileSettings(app: app).tabItem { Label("Profile", systemImage: "person.crop.circle") }
            PrivacySettings(app: app).tabItem { Label("Privacy", systemImage: "hand.raised") }
            SecuritySettings(app: app).tabItem { Label("Security", systemImage: "lock.shield") }
            NetworkSettings(app: app).tabItem { Label("Network", systemImage: "network") }
            if app.profile?.isAdmin == true {
                ServerSettings(app: app).tabItem { Label("Server", systemImage: "server.rack") }
            }
            AboutSettings(app: app).tabItem { Label("About", systemImage: "info.circle") }
        }
        .frame(width: 560, height: 480)
    }
}

struct ProfileSettings: View {
    let app: AppModel
    @State private var name = ""
    @State private var avatar: Data?

    var body: some View {
        Form {
            Section {
                HStack(spacing: 16) {
                    Button(action: pick) {
                        Avatar(id: app.profile?.userID ?? Data(), name: name, image: avatar, size: 72)
                    }
                    .buttonStyle(PressableStyle())
                    VStack(alignment: .leading, spacing: 6) {
                        TextField("Name", text: $name)
                            .textFieldStyle(.roundedBorder)
                            .onSubmit(save)
                        Text("Shared only with your friends, inside encrypted chats.")
                            .font(.system(size: 11.5))
                            .foregroundStyle(.secondary)
                    }
                }
                HStack {
                    if avatar != nil { Button("Remove photo") { avatar = nil; save() } }
                    Spacer()
                    Button("Save", action: save).buttonStyle(.borderedProminent)
                }
            }
            Section("Your fingerprint") {
                Text(app.profile?.fingerprint ?? "")
                    .font(Typo.mono)
                    .textSelection(.enabled)
                Text("Friends see this when they verify you.")
                    .font(.system(size: 11.5))
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .onAppear {
            name = app.profile?.name ?? ""
            avatar = app.profile?.avatar
        }
    }

    private func pick() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.image]
        panel.begin { response in
            guard response == .OK, let url = panel.url, let file = try? MediaPrep.prepare(url: url) else { return }
            Task { @MainActor in
                avatar = file.thumbnail
                save()
            }
        }
    }

    private func save() {
        app.perform {
            try app.core.setProfile(name: name, avatar: avatar)
            app.reloadProfile()
        }
    }
}

struct PrivacySettings: View {
    @Bindable var app: AppModel
    @Bindable var prefs: Preferences

    init(app: AppModel) {
        self.app = app
        prefs = app.prefs
    }

    var body: some View {
        Form {
            Section("Messages") {
                Toggle("Send read receipts", isOn: settingBinding(\.readReceipts))
                Toggle("Show when I'm typing", isOn: settingBinding(\.typingIndicators))
                Picker("Default disappearing timer", selection: settingBinding(\.defaultTimerSeconds)) {
                    ForEach(DisappearingTimer.choices, id: \.self) { Text(DisappearingTimer.label($0)).tag($0) }
                }
                Toggle("Download photos automatically", isOn: settingBinding(\.autoDownloadImages))
            }
            Section {
                Picker("Notifications show", selection: $prefs.notificationLevel) {
                    ForEach(NotificationLevel.allCases) { level in
                        Text("\(level.title) — \(level.example)").tag(level)
                    }
                }
                if prefs.notificationLevel == .fullPreview {
                    Label("macOS keeps its own copy of notification text, outside Asiri's encryption.", systemImage: "exclamationmark.triangle")
                        .font(.system(size: 11.5))
                        .foregroundStyle(Palette.amber)
                }
            } header: {
                Text("Notifications")
            }
            Section("Appearance") {
                HStack {
                    Text("Text size")
                    Slider(value: $prefs.textScale, in: 0.85 ... 1.4, step: 0.05)
                    Text("\(Int(prefs.textScale * 100))%").monospacedDigit().frame(width: 44)
                }
                Toggle("Chat wallpaper", isOn: $prefs.showWallpaper)
            }
        }
        .formStyle(.grouped)
    }

    private func settingBinding<T>(_ key: WritableKeyPath<AppSettings, T>) -> Binding<T> {
        Binding(
            get: { app.settings[keyPath: key] },
            set: { value in
                var s = app.settings
                s[keyPath: key] = value
                app.saveSettings(s)
            }
        )
    }
}

struct SecuritySettings: View {
    let app: AppModel
    @Bindable var prefs: Preferences
    @State private var pin = ""
    @State private var duress = ""
    @State private var confirmWipe = false
    @State private var protection: KeyVault.Protection?

    init(app: AppModel) {
        self.app = app
        prefs = app.prefs
    }

    var body: some View {
        Form {
            Section {
                Toggle(isOn: Binding(get: { prefs.appLockEnabled }, set: setAppLock)) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Require Touch ID or password")
                        Text("Your messages can't be decrypted until you unlock.").font(.system(size: 11.5)).foregroundStyle(.secondary)
                    }
                }
                .disabled(!AppLock.isAvailable)
                Picker("Lock after", selection: $prefs.autoLockMinutes) {
                    Text("1 minute away").tag(1)
                    Text("15 minutes away").tag(15)
                    Text("1 hour away").tag(60)
                    Text("Only when quitting").tag(0)
                }
                .disabled(!prefs.appLockEnabled && !PINStore().hasPIN)
                Toggle("Hide chats when Asiri isn't in front", isOn: $prefs.hideWhenInactive)
            } header: {
                Text("App lock")
            } footer: {
                Text(protection == .secureEnclave ? "Your database key is protected by the Secure Enclave." : "This build keeps your database key in the login keychain.")
            }
            Section {
                SecureField("App PIN (optional)", text: $pin)
                SecureField("Duress PIN", text: $duress)
                HStack {
                    Text("Entering the duress PIN on the lock screen silently erases Asiri.")
                        .font(.system(size: 11.5))
                        .foregroundStyle(.secondary)
                    Spacer()
                    Button("Save PINs") {
                        let store = PINStore()
                        store.setPIN(pin.isEmpty ? nil : pin)
                        store.setDuressPIN(duress.isEmpty ? nil : duress)
                        pin = ""
                        duress = ""
                        app.show(.success, "PINs saved")
                    }
                    .disabled(!duress.isEmpty && duress == pin)
                }
            } header: {
                Text("PIN")
            }
            Section("Emergency") {
                Button("Erase Asiri Now…", role: .destructive) { confirmWipe = true }
                Text("Destroys the key that protects your messages, then deletes everything. This can't be undone.")
                    .font(.system(size: 11.5))
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .onAppear { protection = KeyVault().protection }
        .confirmationDialog("Erase everything on this Mac?", isPresented: $confirmWipe) {
            Button("Erase Asiri", role: .destructive) { Task { await app.panicWipe() } }
        } message: {
            Text("All chats, contacts and keys on this Mac are destroyed. Your friends keep their copies.")
        }
    }

    private func setAppLock(_ on: Bool) {
        Task {
            guard await AppLock.authenticate(reason: on ? "turn on app lock" : "turn off app lock") else { return }
            do {
                try KeyVault().setAppLock(on, reason: "change app lock")
                prefs.appLockEnabled = on
                protection = KeyVault().protection
            } catch {
                app.show(error)
            }
        }
    }
}

struct NetworkSettings: View {
    let app: AppModel

    var body: some View {
        Form {
            Section("Connection") {
                LabeledContent("Status") {
                    HStack(spacing: 6) {
                        StatusDot(app.connection)
                        Text(app.connection.label)
                    }
                }
                LabeledContent("Server", value: app.profile?.relayAddress ?? "Not connected")
                LabeledContent("Route", value: app.profile?.isDirectTransport == true ? "Direct (development only)" : "Tor onion service")
                Button("Reconnect now") { app.core.notifyNetworkChange() }
            }
            if app.profile?.isDirectTransport == true {
                Section {
                    Label("This Mac reaches its server without Tor. Use this only for development.", systemImage: "exclamationmark.triangle")
                        .foregroundStyle(Palette.amber)
                }
            }
            Section("Calls") {
                Text("Call media goes directly to your server's LiveKit, encrypted end to end with a key only the people in the call have.")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }
}

struct ServerSettings: View {
    let app: AppModel
    @State private var status: ServerStatus?
    @State private var loading = false

    var body: some View {
        Form {
            if let status {
                Section("Usage") {
                    LabeledContent("Encrypted files stored", value: Formatting.bytes(status.storedBytes))
                    LabeledContent("Messages waiting", value: "\(status.queuedEnvelopes)")
                    LabeledContent("Who can invite", value: status.invitePolicy == "members" ? "Everyone" : "Admins")
                }
                Section("Devices") {
                    ForEach(status.devices) { device in
                        HStack {
                            Image(systemName: device.isAdmin ? "star.fill" : "laptopcomputer")
                                .foregroundStyle(device.isAdmin ? Palette.amber : Color.secondary)
                            Text(Formatting.grouped(device.id.prefix(6).map { String(format: "%02x", $0) }.joined()))
                                .font(Typo.mono)
                            if device.id == app.profile?.deviceID { Text("this Mac").foregroundStyle(.secondary) }
                            Spacer()
                            Text(label(device.state)).foregroundStyle(device.state == .revoked ? Palette.danger : Color.secondary)
                            if device.state != .revoked, device.id != app.profile?.deviceID {
                                Button("Revoke", role: .destructive) { revoke(device) }
                            }
                        }
                    }
                }
            } else {
                ProgressView("Loading…")
            }
        }
        .formStyle(.grouped)
        .task { await load() }
    }

    private func label(_ s: ServerDevice.State) -> String {
        switch s {
        case .pending: "invited"
        case .active: "active"
        case .revoked: "revoked"
        }
    }

    private func load() async {
        do { status = try await app.core.serverStatus() } catch { app.show(error) }
    }

    private func revoke(_ device: ServerDevice) {
        Task {
            do {
                try await app.core.revokeDevice(device.id)
                await load()
            } catch {
                app.show(error)
            }
        }
    }
}

struct AboutSettings: View {
    let app: AppModel
    @State private var checking = false

    var body: some View {
        VStack(spacing: 14) {
            AsiriMark(size: 76)
            Text("Àṣírí").font(.system(size: 28, weight: .heavy, design: .rounded))
            Text("Between us.").foregroundStyle(.secondary)
            Text("Version \(Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "dev") (\(Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "0"))")
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
            if let update = app.updateAvailable {
                VStack(spacing: 8) {
                    Label("Asiri \(update.version) is available", systemImage: "arrow.down.circle.fill")
                        .foregroundStyle(Palette.accent)
                    if !update.notes.isEmpty {
                        Text(update.notes)
                            .font(.system(size: 12))
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                            .frame(maxWidth: 380)
                    }
                    Button(app.installingUpdate ? "Downloading…" : "Install Update") {
                        Task { await app.installUpdate() }
                    }
                    .buttonStyle(PrimaryButtonStyle())
                    .disabled(app.installingUpdate)
                }
            }
            Button {
                checking = true
                Task {
                    do {
                        if let u = try await app.core.checkForUpdate() {
                            app.updateAvailable = u
                        } else {
                            app.show(.success, "Asiri is up to date")
                        }
                    } catch {
                        app.show(error)
                    }
                    checking = false
                }
            } label: {
                HStack {
                    if checking { ProgressView().controlSize(.small) }
                    Text("Check for updates")
                }
            }
            .buttonStyle(SecondaryButtonStyle())
            Text("Messages are end-to-end encrypted with post-quantum keys (MLS with X-Wing). Your server only ever sees ciphertext.")
                .font(.system(size: 11.5))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 380)
        }
        .padding(30)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
