import Foundation

/// The protocol layer between the views and paddock. The stores call this,
/// never the runner or a socket. `CLIPaddockService` implements it with the
/// paddock CLI. Tests use it with `ReplayRunner`.
public protocol PaddockService: AnyObject, Sendable {
    /// `paddock --version`, bounded by `LauncherProtocol.limits`.
    func version() async throws -> String
    /// `paddock --launcher-protocol`, bounded by `LauncherProtocol.limits`.
    /// The output as it is: `LauncherProtocol.check` reads it.
    func launcherProtocol() async throws -> CommandOutput
    func macvmStatus() async throws -> MacvmStatusJSON
    func doctor() async throws -> DoctorJSON
    func listApps() async throws -> [AppJSON]
    func appStatus(_ app: String) async throws -> AppStatusJSON
    func approvals() async throws -> ApprovalsJSON
    /// `paddock app add PATH --dry-run --json`.
    func precheck(_ path: String) async throws -> PrecheckJSON
    /// Any command that answers with a `ResultJSON`.
    func perform(_ args: [String]) async throws -> ResultJSON
    /// A command without JSON output. Throws on a non-zero exit.
    func execute(_ args: [String]) async throws
    /// A command whose stdout is plain text.
    func text(_ args: [String]) async throws -> String
    /// The exit status and both outputs, for a command whose refusal is
    /// JSON with a non-zero exit, or text on stderr.
    func raw(_ args: [String]) async throws -> CommandOutput
    /// A long-lived command whose stdout is plain text lines.
    func lines(_ args: [String]) -> AsyncThrowingStream<String, Error>
    /// `paddock events --follow --json`, decoded.
    func events() -> AsyncThrowingStream<PaddockEvent, Error>
    /// `paddock macvm setup --json ...`, decoded.
    func setup(_ args: [String]) -> AsyncThrowingStream<SetupJSON, Error>
}

public final class CLIPaddockService: PaddockService, @unchecked Sendable {
    public let runner: CommandRunner

    public init(runner: CommandRunner) {
        self.runner = runner
    }

    public func version() async throws -> String {
        let out = try await runner.run(PaddockArgs.version, limits: LauncherProtocol.limits)
        if out.status != 0 { throw CommandError.failed(status: out.status, line: CLIPaddockService.lastLine(out.stderr)) }
        return out.stdoutText
    }

    public func launcherProtocol() async throws -> CommandOutput {
        try await runner.run(PaddockArgs.launcherProtocol, limits: LauncherProtocol.limits)
    }

    public func macvmStatus() async throws -> MacvmStatusJSON {
        let out = try await runner.run(PaddockArgs.macvmStatus)
        if out.status != 0 { throw CommandError.failed(status: out.status, line: CLIPaddockService.lastLine(out.stderr)) }
        do {
            return try LibraryDecode.macvmStatus(out.stdout)
        } catch {
            throw CommandError.badJSON(output: String(out.stdoutText.prefix(200)))
        }
    }
    public func doctor() async throws -> DoctorJSON { try await json(PaddockArgs.doctor) }
    public func listApps() async throws -> [AppJSON] {
        let out = try await runner.run(PaddockArgs.appList)
        if out.status != 0 { throw CommandError.failed(status: out.status, line: CLIPaddockService.lastLine(out.stderr)) }
        do {
            return try LibraryDecode.apps(out.stdout)
        } catch {
            throw CommandError.badJSON(output: String(out.stdoutText.prefix(200)))
        }
    }
    public func appStatus(_ app: String) async throws -> AppStatusJSON { try await json(PaddockArgs.appStatus(app)) }
    public func approvals() async throws -> ApprovalsJSON {
        let out = try await runner.run(PaddockArgs.approvals)
        if let value = try? PaddockJSON.decoder.decode(ApprovalsJSON.self, from: out.stdout),
           out.status == 0 || (out.status == 2 && !value.complete) {
            return value
        }
        if out.status != 0 { throw CommandError.failed(status: out.status, line: CLIPaddockService.lastLine(out.stderr)) }
        throw CommandError.badJSON(output: String(out.stdoutText.prefix(200)))
    }

    /// A refused precheck still prints JSON, maybe with a non-zero exit.
    public func precheck(_ path: String) async throws -> PrecheckJSON {
        let out = try await runner.run(PaddockArgs.precheck(path))
        if let report = LibraryDecode.addReport(out.stdout) { return PrecheckJSON(report: report) }
        if let value = try? PaddockJSON.decoder.decode(PrecheckJSON.self, from: out.stdout) { return value }
        if out.status != 0 { throw CommandError.failed(status: out.status, line: CLIPaddockService.lastLine(out.stderr)) }
        throw CommandError.badJSON(output: String(out.stdoutText.prefix(200)))
    }

    /// A refusal is a normal answer: paddock prints `ok: false` with a line
    /// and may exit non-zero. The JSON wins over the exit status.
    public func perform(_ args: [String]) async throws -> ResultJSON {
        let out = try await runner.run(args)
        if let result = try? PaddockJSON.decoder.decode(ResultJSON.self, from: out.stdout) {
            return result
        }
        if out.status != 0 { throw CommandError.failed(status: out.status, line: CLIPaddockService.lastLine(out.stderr)) }
        throw CommandError.badJSON(output: String(out.stdoutText.prefix(200)))
    }

    public func execute(_ args: [String]) async throws {
        let out = try await runner.run(args)
        if out.status != 0 { throw CommandError.failed(status: out.status, line: CLIPaddockService.lastLine(out.stderr)) }
    }

    public func text(_ args: [String]) async throws -> String {
        let out = try await runner.run(args)
        if out.status != 0 { throw CommandError.failed(status: out.status, line: CLIPaddockService.lastLine(out.stderr)) }
        return out.stdoutText
    }

    public func lines(_ args: [String]) -> AsyncThrowingStream<String, Error> {
        runner.lines(args)
    }

    public func raw(_ args: [String]) async throws -> CommandOutput {
        try await runner.run(args)
    }

    public func events() -> AsyncThrowingStream<PaddockEvent, Error> {
        CLIPaddockService.decoded(runner.lines(PaddockArgs.events)) { PaddockEvent.decode(line: $0) }
    }

    public func setup(_ args: [String]) -> AsyncThrowingStream<SetupJSON, Error> {
        CLIPaddockService.decoded(runner.lines(args)) { LibraryDecode.setupLine($0) }
    }

    func json<T: Decodable>(_ args: [String]) async throws -> T {
        let out = try await runner.run(args)
        if out.status != 0 { throw CommandError.failed(status: out.status, line: CLIPaddockService.lastLine(out.stderr)) }
        do {
            return try PaddockJSON.decoder.decode(T.self, from: out.stdout)
        } catch {
            throw CommandError.badJSON(output: String(out.stdoutText.prefix(200)))
        }
    }

    /// paddock's last non-empty stderr line: its own words for the failure.
    public static func lastLine(_ text: String) -> String {
        text.split(separator: "\n").last { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
            .map { $0.trimmingCharacters(in: .whitespaces) } ?? ""
    }

    static func decoded<T: Sendable>(_ source: AsyncThrowingStream<String, Error>,
                                     _ transform: @escaping @Sendable (String) -> T?) -> AsyncThrowingStream<T, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    for try await line in source {
                        if let value = transform(line) { continuation.yield(value) }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}
