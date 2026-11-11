import Foundation

/// One row of the review checklist in the Add App sheet.
public struct AddCheck: Equatable, Sendable, Identifiable {
    public enum Kind: String, Equatable, Sendable, CaseIterable {
        case bundle, appStore, golden, space, name
    }

    public enum Status: Equatable, Sendable {
        case pass, fail
    }

    public enum Action: Equatable, Sendable {
        case openWebsite(String)
        case build
        case manageStorage
    }

    public var kind: Kind
    public var status: Status
    public var title: UIText
    /// The pass text, or paddock's line for a failed row.
    public var detail: UIText
    public var action: Action?

    public var id: String { kind.rawValue }

    public var symbol: String { status == .pass ? "checkmark.circle.fill" : "xmark.circle.fill" }
    public var tint: SymbolTint { status == .pass ? .green : .red }
    public var accessibilityLabel: UIText { status == .pass ? UIText("Passed") : UIText("Failed") }

    public static func title(_ kind: Kind) -> UIText {
        switch kind {
        case .bundle: return UIText("App bundle")
        case .appStore: return UIText("Not from the App Store")
        case .golden: return UIText("Base image")
        case .space: return UIText("Free space")
        case .name: return UIText("Name")
        }
    }
}

/// What the Add App sheet shows after paddock's dry run: the app, the
/// checklist, the memory choice and whether `Add` is enabled.
public struct AddReview: Equatable, Sendable {
    public var path: String
    public var name: String
    public var displayName: String
    public var version: String?
    public var bytes: Int64?
    public var checks: [AddCheck]
    /// The profile name when this bundle is in paddock already.
    public var existing: String?
    /// A failure without a report: paddock's line.
    public var failure: String?
    public var memoryOptions: [Int]
    public var defaultMemory: Int

    public var canAdd: Bool {
        failure == nil && existing == nil && !checks.isEmpty && checks.allSatisfy { $0.status == .pass }
    }

    /// The first failed row: the reason `Add` is off.
    public var firstFailure: AddCheck? { checks.first { $0.status == .fail } }

    public static let defaultOptions = [4, 6, 8]

    /// The memory choices: 4, 6 and 8 GB, at most paddock's `max_mib`.
    public static func memoryOptions(_ memory: AddMemoryJSON?) -> [Int]? {
        guard let memory else { return nil }
        guard let max = memory.maxMib else { return defaultOptions }
        let options = defaultOptions.filter { Int64($0) * 1024 <= max }
        return options.isEmpty ? [defaultOptions[0]] : options
    }

    static func defaultMemory(_ memory: AddMemoryJSON?, options: [Int]) -> Int {
        if let mib = memory?.defaultMib {
            let gb = Int((Double(mib) / 1024).rounded())
            if options.contains(gb) { return gb }
        }
        return options.contains(6) ? 6 : (options.last ?? 4)
    }

    /// From paddock's add report.
    public static func of(report: AddReportJSON, path: String) -> AddReview {
        let refusals = report.refusals ?? []
        func refusal(_ codes: Set<String>) -> AddRefusalJSON? { refusals.first { codes.contains($0.code) } }
        var checks: [AddCheck] = []
        if let r = refusal(["not_a_bundle"]) {
            checks.append(fail(.bundle, r.text))
        } else {
            checks.append(pass(.bundle, report.version.map { UIText("Version %@", $0) } ?? UIText("An app bundle")))
        }
        if let r = refusal(["app_store"]) {
            checks.append(fail(.appStore, r.text))
        } else if report.appStore == true {
            checks.append(fail(.appStore, AddVerdict.appStore(website: nil).message!.english))
        } else {
            checks.append(pass(.appStore, UIText("Downloaded from the maker")))
        }
        if let r = refusal(["no_golden"]) {
            checks.append(fail(.golden, r.text, action: .build))
        } else {
            checks.append(pass(.golden, report.golden == "sealed" ? UIText("Ready") : UIText("Building")))
        }
        if let r = refusal(["low_space"]) {
            checks.append(fail(.space, r.text, action: .manageStorage))
        } else if let space = report.space {
            checks.append(pass(.space, UIText("%@ free, %@ needed", Format.gb(space.free), Format.gb(space.need))))
        }
        if let r = refusal(["bad_name", "name_taken"]) {
            checks.append(fail(.name, r.text))
        }
        // A refusal code this launcher does not know still blocks the add.
        let known: Set<String> = ["not_a_bundle", "app_store", "no_golden", "low_space", "bad_name", "name_taken", "bad_memory"]
        for r in refusals where !known.contains(r.code) {
            checks.append(fail(.bundle, r.text))
        }
        let options = memoryOptions(report.memory) ?? defaultOptions
        let name = report.name ?? AddReview.slugBase(path)
        return AddReview(path: path, name: name, displayName: report.displayName ?? name, version: report.version,
                         bytes: report.bytes, checks: checks, existing: report.existing, failure: report.error,
                         memoryOptions: options, defaultMemory: defaultMemory(report.memory, options: options))
    }

    /// From the older precheck shape of the launcher fixtures.
    public static func of(precheck: PrecheckJSON, path: String) -> AddReview {
        var checks: [AddCheck] = [pass(.bundle, precheck.version.map { UIText("Version %@", $0) } ?? UIText("An app bundle"))]
        switch AddVerdict.of(precheck) {
        case .appStore(let website):
            checks.append(fail(.appStore, AddVerdict.appStore(website: website).message!.english,
                               action: website.map { .openWebsite($0) }))
        case .noGolden:
            checks.append(pass(.appStore, UIText("Downloaded from the maker")))
            checks.append(fail(.golden, AddVerdict.noGolden.message!.english, action: .build))
        case .lowSpace(let need, let free):
            checks.append(pass(.appStore, UIText("Downloaded from the maker")))
            checks.append(pass(.golden, UIText("Ready")))
            checks.append(fail(.space, AddVerdict.lowSpace(needBytes: need, freeBytes: free).message!.english,
                               action: .manageStorage))
        case .failed(let line):
            checks.append(fail(.bundle, line))
        case .ok:
            checks.append(pass(.appStore, UIText("Downloaded from the maker")))
            checks.append(pass(.golden, UIText("Ready")))
            if let need = precheck.needBytes, let free = precheck.freeBytes {
                checks.append(pass(.space, UIText("%@ free, %@ needed", Format.gb(free), Format.gb(need))))
            }
        }
        let options = precheck.memoryOptionsGb ?? defaultOptions
        return AddReview(path: path, name: precheck.name, displayName: precheck.name, version: precheck.version,
                         bytes: precheck.sizeBytes, checks: checks, existing: nil, failure: nil,
                         memoryOptions: options, defaultMemory: options.contains(6) ? 6 : (options.last ?? 4))
    }

    /// A review that could not run: paddock's line.
    public static func failed(path: String, line: String) -> AddReview {
        let name = slugBase(path)
        return AddReview(path: path, name: name, displayName: name, version: nil, bytes: nil, checks: [],
                         existing: nil, failure: line, memoryOptions: defaultOptions, defaultMemory: 6)
    }

    /// `Notebook` for `/Applications/Notebook.app`.
    public static func slugBase(_ path: String) -> String {
        let last = path.split(separator: "/").last.map(String.init) ?? path
        return last.hasSuffix(".app") ? String(last.dropLast(4)) : last
    }

    static func pass(_ kind: AddCheck.Kind, _ detail: UIText) -> AddCheck {
        AddCheck(kind: kind, status: .pass, title: AddCheck.title(kind), detail: detail, action: nil)
    }

    static func fail(_ kind: AddCheck.Kind, _ line: String, action: AddCheck.Action? = nil) -> AddCheck {
        AddCheck(kind: kind, status: .fail, title: AddCheck.title(kind), detail: UIText(key: "%@", args: [line]), action: action)
    }
}

/// The outcome of `Add` in the review sheet.
public enum AddOutcome: Equatable, Sendable {
    case added(key: String)
    case refused(String)
}
