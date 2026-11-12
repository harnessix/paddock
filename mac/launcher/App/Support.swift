import AppKit
import PaddockKit
import SwiftUI
import UniformTypeIdentifiers

// MARK: - Localization

extension UIText {
    /// The text in the user's language, with the values filled in. The key
    /// is in Localizable.xcstrings.
    var localized: String {
        let format = Bundle.main.localizedString(forKey: key, value: key, table: nil)
        return UIText.fill(format, args)
    }
}

extension Text {
    /// A text that PaddockKit built: localized through the catalog.
    init(_ text: UIText) {
        self.init(verbatim: text.localized)
    }
}

extension View {
    /// Opaque system label color. SwiftUI's primary and secondary styles are
    /// vibrant on macOS, and the accessibility audit counts them as too faint.
    func labelInk() -> some View {
        foregroundStyle(Color(nsColor: .labelColor))
    }

    /// Menus and popup pickers have no press action, so the audit reports
    /// them as missing one. The control still opens from its own click.
    func menuAuditAction() -> some View {
        accessibilityAction(named: Text("Show menu")) {}
    }
}

/// A stand-in for `ContentUnavailableView`, with the same layout: a large
/// symbol, a bold title, one sentence and one action. That view paints its
/// title and description with secondary and tertiary colors, which fail the
/// contrast audit, and a parent `foregroundStyle` does not override them.
@MainActor
struct PlainEmpty<Title: View, Description: View, Actions: View>: View {
    let title: Title
    let description: Description
    let actions: Actions

    init(@ViewBuilder title: () -> Title,
         @ViewBuilder description: () -> Description,
         @ViewBuilder actions: () -> Actions) {
        self.title = title()
        self.description = description()
        self.actions = actions()
    }

    var body: some View {
        VStack(spacing: 10) {
            title
                .labelStyle(EmptyStateLabelStyle())
                .labelInk()
            description
                .labelInk()
                .fixedSize(horizontal: false, vertical: true)
            actions
                .padding(.top, 6)
        }
        .multilineTextAlignment(.center)
        .frame(maxWidth: 420)
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .labelInk()
    }
}

/// The title of an empty state: the symbol large over the title, as
/// `ContentUnavailableView` draws it. The symbol only decorates.
struct EmptyStateLabelStyle: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        VStack(spacing: 12) {
            configuration.icon
                .font(.system(size: 44, weight: .regular))
                .symbolRenderingMode(.hierarchical)
                .foregroundStyle(Color.secondary)
                .imageScale(.large)
            configuration.title
                .font(.title2.bold())
        }
    }
}

// MARK: - Status symbols

extension SymbolTint {
    var color: Color {
        switch self {
        case .secondary: return .secondary
        case .green: return .green
        case .orange: return .orange
        case .red: return .red
        }
    }
}

/// An SF Symbol with a system tint and a spoken state word. Color is never
/// the only signal: the label carries the state.
@MainActor
struct StatusSymbol: View {
    let name: String
    let tint: SymbolTint
    let label: UIText

    var body: some View {
        Image(systemName: name)
            .foregroundStyle(tint.color)
            .accessibilityLabel(Text(label))
    }
}

/// The status of an app: a symbol, or a small spinner while it changes.
@MainActor
struct AppStatusView: View {
    let presentation: StatusPresentation

    var body: some View {
        if presentation.showsProgress {
            ProgressView()
                .controlSize(.small)
                .accessibilityLabel(Text(presentation.accessibilityLabel))
        } else if let symbol = presentation.symbol {
            StatusSymbol(name: symbol, tint: presentation.tint, label: presentation.accessibilityLabel)
        }
    }
}

/// An SF Symbol that only decorates: the text next to it says the same,
/// so VoiceOver skips it.
@MainActor
struct DecorativeSymbol: View {
    let name: String

    var body: some View {
        Image(systemName: name).accessibilityHidden(true)
    }
}

/// A problem in a sheet or a form: a warning symbol, then the text in the
/// label color. Red text is under 4.5:1 on a light background, so the
/// symbol carries the color and the text stays readable.
@MainActor
struct ProblemLine<Content: View>: View {
    let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
                .accessibilityHidden(true)
            content
        }
    }
}

// MARK: - Error shape

/// The error shape: symbol, one-line title, one-line cause and next step,
/// a primary button and `Show Log`.
@MainActor
struct ErrorShape: View {
    let error: LauncherError
    var primary: () -> Void = {}
    var showLog: (() -> Void)?
    var stopChoices: [String] = []
    var stop: (String) -> Void = { _ in }

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: error.symbol)
                .font(.title2)
                .foregroundStyle(error.symbol == "info.circle" ? Color.secondary : Color.orange)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 6) {
                if let title = error.title {
                    Text(title).font(.headline)
                }
                if let body = error.body {
                    Text(body).fixedSize(horizontal: false, vertical: true)
                }
                if let detail = error.detail {
                    Text(verbatim: detail)
                }
                if let command = error.command {
                    CopyField(command: command)
                }
                HStack {
                    primaryButton
                    if error.showsLogButton, let showLog {
                        Button("Show Log", action: showLog)
                    }
                }
            }
            Spacer(minLength: 0)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("error-shape")
    }

    @ViewBuilder var primaryButton: some View {
        if error.primary == .stopPicker {
            Menu {
                ForEach(stopChoices, id: \.self) { name in
                    Button(name) { stop(name) }
                }
            } label: {
                Text(LauncherError.Primary.stopPicker.title!)
            }
            .menuAuditAction()
            .fixedSize()
        } else if let title = error.primary.title {
            Button(action: primary) { Text(title) }
        }
    }
}

// MARK: - Small controls

/// A command in a read-only field with a `Copy` button. The launcher never
/// runs it: the user pastes it in Terminal.
@MainActor
struct CopyField: View {
    let command: String
    @State private var copied = false

    var body: some View {
        HStack {
            TextField("Command", text: .constant(command))
                .textFieldStyle(.roundedBorder)
                .font(.system(.body, design: .monospaced))
                .accessibilityIdentifier("command-field")
                .labelsHidden()
            Button {
                Pasteboard.copy(command)
                copied = true
            } label: {
                copied ? Text("Copied") : Text("Copy")
            }
            .accessibilityIdentifier("copy-button")
        }
    }
}

/// The `Terminal` row under a sheet when "Show the paddock command for each
/// action" is on: the equivalent command, for support.
@MainActor
struct CommandEcho: View {
    let args: [String]
    @AppStorage(SettingsKey.showCommands, store: LauncherDefaults.store) private var showCommands = false

    var body: some View {
        if showCommands {
            LabeledContent("Terminal") {
                Text(verbatim: PaddockArgs.display(args))
                    .font(.system(.caption, design: .monospaced))
                    .textSelection(.enabled)
            }
        }
    }
}

/// The toast line at the bottom of a window.
@MainActor
struct ToastBar: View {
    let toast: Toast?

    var body: some View {
        if let toast {
            HStack {
                Image(systemName: "checkmark.circle").accessibilityHidden(true)
                Text(toast.text)
                Spacer()
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(.bar)
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("toast")
        }
    }
}

/// `Live updates paused · Refresh` while the event stream is down.
@MainActor
struct StreamStatusLine: View {
    @Environment(LauncherStore.self) private var store

    var body: some View {
        if !store.streamHealthy {
            HStack(spacing: 6) {
                Image(systemName: "wifi.exclamationmark").accessibilityHidden(true)
                Text("Live updates paused ·")
                Button("Refresh") { Task { await store.refresh() } }
                    .buttonStyle(.link)
                    .disabled(!store.binary.isUsable)
                Spacer()
            }
            .font(.callout)
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .background(.bar)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("stream-status")
        }
    }
}

/// The icons of app bundles, cached by path and modification date: a list
/// redraws often, and `NSWorkspace` reads the bundle at each call.
@MainActor
enum AppIconCache {
    private struct Entry {
        let date: Date?
        let image: NSImage
    }

    private static var entries: [String: Entry] = [:]

    /// The bundle's icon, or nil when no bundle is at `path`.
    static func icon(_ path: String) -> NSImage? {
        guard FileManager.default.fileExists(atPath: path) else {
            entries[path] = nil
            return nil
        }
        let date = (try? FileManager.default.attributesOfItem(atPath: path))?[.modificationDate] as? Date
        if let entry = entries[path], entry.date == date { return entry.image }
        let image = NSWorkspace.shared.icon(forFile: path)
        entries[path] = Entry(date: date, image: image)
        return image
    }

    /// The generic application icon of Finder.
    static let generic: NSImage = NSWorkspace.shared.icon(for: .applicationBundle)
}

/// The icon sizes of the launcher: the detail header, a question card, the
/// sidebar and the menu.
enum IconSize {
    static let header: CGFloat = 64
    static let question: CGFloat = 32
    static let sidebar: CGFloat = 24
    static let menu: CGFloat = 16
}

/// The icon of an app bundle. When the bundle is gone, the generic app icon
/// at half strength with a question mark: never an empty square.
@MainActor
struct AppIcon: View {
    let path: String?
    var size: CGFloat = IconSize.sidebar

    var body: some View {
        if let path, let image = AppIconCache.icon(path) {
            Image(nsImage: image)
                .resizable()
                .interpolation(.high)
                .frame(width: size, height: size)
                .accessibilityHidden(true)
        } else {
            Image(nsImage: AppIconCache.generic)
                .resizable()
                .interpolation(.high)
                .frame(width: size, height: size)
                .opacity(0.5)
                .overlay(alignment: .bottomTrailing) {
                    Image(systemName: "questionmark.circle.fill")
                        .resizable()
                        .symbolRenderingMode(.palette)
                        .foregroundStyle(Color.white, Color.gray)
                        .frame(width: max(8, size * 0.4), height: max(8, size * 0.4))
                }
                .accessibilityHidden(true)
        }
    }
}

/// An app icon with the state as a dot on its lower right corner, as the
/// sidebar shows it. The dot pulses only while the app starts.
@MainActor
struct AppIconWithStatus: View {
    let path: String?
    let presentation: StatusPresentation
    var size: CGFloat = IconSize.sidebar
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        AppIcon(path: path, size: size)
            .overlay(alignment: .bottomTrailing) {
                Image(systemName: "circle.fill")
                    .font(.system(size: max(7, size * 0.34)))
                    .foregroundStyle(presentation.dotColor)
                    .symbolEffect(.pulse, isActive: presentation.pulses && !reduceMotion)
                    .background(Circle().fill(Color(nsColor: .windowBackgroundColor)).padding(-1.5))
                    .offset(x: 2, y: 2)
                    .accessibilityHidden(true)
            }
            .padding(.trailing, 2)
            .padding(.bottom, 2)
    }
}

/// The state of an app as a pill: the symbol, tinted, and the word, both on
/// a quiet capsule. Only the symbol carries color. While the app starts,
/// a small spinner and the phase.
@MainActor
struct StatusPill: View {
    let presentation: StatusPresentation
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: 5) {
            if presentation.showsProgress {
                ProgressView()
                    .controlSize(.mini)
                    .accessibilityHidden(true)
                Text(presentation.kind == .starting ? presentation.text : presentation.word)
                    .contentTransition(.opacity)
            } else {
                if let symbol = presentation.symbol {
                    Image(systemName: symbol)
                        .foregroundStyle(presentation.tint.color)
                        .accessibilityHidden(true)
                }
                Text(presentation.word)
            }
        }
        .font(.callout)
        .labelInk()
        .lineLimit(1)
        .padding(.horizontal, 8)
        .padding(.vertical, 3)
        .background(.fill.quaternary, in: Capsule())
        .animation(reduceMotion ? nil : .default, value: presentation)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(presentation.accessibilityLabel))
    }
}

extension StatusPresentation {
    /// The dot on the sidebar icon: green Running, orange Error, red
    /// Blocked, gray else. The text under the name says the same.
    var dotColor: Color {
        switch kind {
        case .running: return .green
        case .error: return .orange
        case .blocked: return .red
        default: return .gray
        }
    }
}

// MARK: - System helpers

enum Pasteboard {
    static func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}

enum SystemLinks {
    static let storage = URL(string: "x-apple.systempreferences:com.apple.settings.Storage")!
    static let softwareUpdate = URL(string: "x-apple.systempreferences:com.apple.Software-Update-Settings.extension")!

    static func open(_ url: URL) { NSWorkspace.shared.open(url) }

    static func reveal(_ path: String) {
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }

    static func manageStorage() { open(storage) }
}

/// Open panels. Debug builds take the answer from the environment, so the
/// UI tests never drive a system panel.
enum Pickers {
    static func override(_ name: String) -> String? {
        #if DEBUG
        return ProcessInfo.processInfo.environment[name]
        #else
        return nil
        #endif
    }

    /// An `.app` from /Applications or ~/Applications.
    @MainActor
    static func app() -> String? {
        if let path = override("PADDOCK_LAUNCHER_PICK_APP") { return path }
        let panel = NSOpenPanel()
        panel.directoryURL = URL(fileURLWithPath: "/Applications")
        panel.allowedContentTypes = [.applicationBundle]
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.treatsFilePackagesAsDirectories = false
        panel.prompt = String(localized: "Choose")
        guard panel.runModal() == .OK, let url = panel.url else { return nil }
        return url.path
    }

    @MainActor
    static func folder(start: String? = nil) -> String? {
        if let path = override("PADDOCK_LAUNCHER_PICK_FOLDER") { return path }
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.canCreateDirectories = true
        if let start { panel.directoryURL = URL(fileURLWithPath: start) }
        guard panel.runModal() == .OK, let url = panel.url else { return nil }
        return url.path
    }

    @MainActor
    static func restoreImage() -> String? {
        if let path = override("PADDOCK_LAUNCHER_PICK_IPSW") { return path }
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "ipsw") ?? .data]
        panel.allowsMultipleSelection = false
        guard panel.runModal() == .OK, let url = panel.url else { return nil }
        return url.path
    }

    /// The reason a folder cannot hold the base image, or nil when it can:
    /// a local APFS volume.
    static func storageProblem(_ path: String) -> LocalizedStringKey? {
        let url = URL(fileURLWithPath: path)
        let values = try? url.resourceValues(forKeys: [.volumeIsLocalKey, .volumeTypeNameKey])
        if values?.volumeIsLocal == false {
            return "Choose a folder on this Mac. paddock cannot keep the base image on a network volume." // localized
        }
        if let type = values?.volumeTypeName, type.lowercased() != "apfs" {
            return "Choose a folder on an APFS volume. paddock copies the base image for each app, which needs APFS." // localized
        }
        return nil
    }
}
