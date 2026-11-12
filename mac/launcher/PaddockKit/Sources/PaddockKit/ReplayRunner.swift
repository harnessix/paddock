import Foundation

/// A fake paddock for tests: it replays recorded outputs and streams and
/// logs every call. It stands in for the binary behind `CommandRunner`, so
/// the real `CLIPaddockService` and the stores run unchanged.
public final class ReplayRunner: CommandRunner, @unchecked Sendable {
    public struct Call: Equatable, Sendable {
        public var time: Date
        public var args: [String]
        public var line: String { args.joined(separator: " ") }
    }

    private let lock = NSLock()
    private var outputs: [String: [CommandOutput]] = [:]
    private var used: [String: Int] = [:]
    private var feeds: [String: Feed] = [:]
    private var _calls: [Call] = []
    public let clock: LauncherClock

    public init(clock: LauncherClock = SystemClock()) {
        self.clock = clock
    }

    /// Answers `args` (or any call that starts with them) with `json`.
    /// Several answers for one key are given in turn, the last one repeats.
    public func on(_ args: [String], json: String, status: Int32 = 0, stderr: String = "") {
        on(args, output: CommandOutput(status: status, stdout: Data(json.utf8), stderr: stderr))
    }

    public func on(_ args: [String], output: CommandOutput) {
        on(prefix: args.joined(separator: " "), output: output)
    }

    /// Answers every call whose line starts with `prefix`.
    public func on(prefix: String, output: CommandOutput) {
        lock.lock()
        outputs[prefix, default: []].append(output)
        lock.unlock()
    }

    /// Forgets the answers for `prefix`, so a test can give new ones.
    public func reset(prefix: String) {
        lock.lock()
        outputs[prefix] = nil
        used[prefix] = nil
        lock.unlock()
    }

    /// The answer of a child that never ends. A bounded run of it ends at
    /// its time limit or at a cancel, as `ProcessRunner` kills the child.
    public static let hang = CommandOutput(status: Int32.min, stdout: Data(), stderr: "fake paddock: hang")

    /// Answers every call that starts with `prefix` with a hang.
    public func hang(prefix: String) {
        on(prefix: prefix, output: ReplayRunner.hang)
    }

    /// Loads a scenario folder of mac/launcher/Fixtures: its map.tsv and
    /// the answer files in ../base. Stream files (.jsonl) are skipped: a
    /// test pushes stream lines through `feed`. The answer `hang` never
    /// ends.
    public static func load(scenario: URL, clock: LauncherClock = SystemClock()) throws -> ReplayRunner {
        let runner = ReplayRunner(clock: clock)
        let base = scenario.deletingLastPathComponent().appendingPathComponent("base")
        let map = try String(contentsOf: scenario.appendingPathComponent("map.tsv"), encoding: .utf8)
        for row in map.split(separator: "\n") where !row.hasPrefix("#") {
            let parts = row.split(separator: "\t", maxSplits: 1).map(String.init)
            guard parts.count == 2 else { continue }
            for item in parts[1].split(separator: ",").map(String.init) {
                var name = item
                var status: Int32 = 0
                if let colon = item.lastIndex(of: ":") {
                    name = String(item[..<colon])
                    status = Int32(item[item.index(after: colon)...]) ?? 0
                }
                if name.hasSuffix(".jsonl") { continue }
                if name == "hang" {
                    runner.hang(prefix: parts[0])
                    continue
                }
                let data = try Data(contentsOf: base.appendingPathComponent(name))
                // A .err file is paddock's stderr, as in the fake script.
                let output = name.hasSuffix(".err")
                    ? CommandOutput(status: status, stdout: Data(), stderr: String(decoding: data, as: UTF8.self))
                    : CommandOutput(status: status, stdout: data, stderr: "")
                runner.on(prefix: parts[0], output: output)
            }
        }
        return runner
    }

    /// The JSON lines of a stream fixture, without the `#sleep` and `#hold`
    /// lines of the fake script.
    public static func streamLines(_ file: URL) throws -> [String] {
        try String(contentsOf: file, encoding: .utf8).split(separator: "\n")
            .map(String.init).filter { !$0.hasPrefix("#") && !$0.isEmpty }
    }

    /// Answers `args` with the contents of a fixture file.
    public func on(_ args: [String], file: URL, status: Int32 = 0) throws {
        on(args, output: CommandOutput(status: status, stdout: try Data(contentsOf: file), stderr: ""))
    }

    public var calls: [Call] {
        lock.lock()
        defer { lock.unlock() }
        return _calls
    }

    public func calls(startingWith prefix: [String]) -> [Call] {
        calls.filter { $0.args.starts(with: prefix) }
    }

    public func run(_ args: [String]) async throws -> CommandOutput {
        answer(args)
    }

    /// The bounded run: the limits as `ProcessRunner` applies them.
    public func run(_ args: [String], limits: RunLimits) async throws -> CommandOutput {
        let output = answer(args)
        if output == ReplayRunner.hang { return try await hold(limits) }
        if output.stdout.count > limits.maxBytes || output.stderr.utf8.count > limits.maxBytes {
            count(hanging: 0, kills: 1)
            throw CommandError.tooLarge
        }
        return output
    }

    /// The kills of a bounded run: past a limit or at a cancel.
    public var kills: Int {
        lock.lock()
        defer { lock.unlock() }
        return _kills
    }

    /// The bounded runs that hang now.
    public var hanging: Int {
        lock.lock()
        defer { lock.unlock() }
        return _hanging
    }

    private var _kills = 0
    private var _hanging = 0

    /// Synchronous, so the lock is not taken in an async context.
    private func count(hanging: Int, kills: Int) {
        lock.lock()
        _hanging += hanging
        _kills += kills
        lock.unlock()
    }

    /// Waits until the time limit on the clock, or until a cancel.
    private func hold(_ limits: RunLimits) async throws -> CommandOutput {
        let end = HangEnd()
        count(hanging: 1, kills: 0)
        defer { count(hanging: -1, kills: 1) }
        let clock = self.clock
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<CommandOutput, Error>) in
                end.attach(continuation)
                Task {
                    try? await clock.sleep(seconds: limits.seconds)
                    end.finish(CommandError.timedOut)
                }
            }
        } onCancel: {
            end.finish(CancellationError())
        }
    }

    /// The one end of a hang: the first of the time limit and the cancel.
    final class HangEnd: @unchecked Sendable {
        private let lock = NSLock()
        private var continuation: CheckedContinuation<CommandOutput, Error>?
        private var error: Error?
        private var done = false

        func attach(_ continuation: CheckedContinuation<CommandOutput, Error>) {
            lock.lock()
            if let error = self.error {
                done = true
                lock.unlock()
                continuation.resume(throwing: error)
                return
            }
            self.continuation = continuation
            lock.unlock()
        }

        func finish(_ error: Error) {
            lock.lock()
            guard !done, self.error == nil else {
                lock.unlock()
                return
            }
            guard let continuation = self.continuation else {
                self.error = error
                lock.unlock()
                return
            }
            done = true
            self.continuation = nil
            lock.unlock()
            continuation.resume(throwing: error)
        }
    }

    /// Logs the call and picks the answer. Synchronous, so the lock is not
    /// held across a suspension point.
    private func answer(_ args: [String]) -> CommandOutput {
        lock.lock()
        defer { lock.unlock() }
        _calls.append(Call(time: clock.now(), args: args))
        let key = bestKey(for: args)
        var answer = CommandOutput(status: 1, stdout: Data(), stderr: "fake paddock: no fixture for \(args.joined(separator: " "))")
        if let key, let list = outputs[key], !list.isEmpty {
            let index = min(used[key, default: 0], list.count - 1)
            used[key] = index + 1
            answer = list[index]
        }
        return answer
    }

    /// The longest registered key that the call line starts with, as the
    /// fake paddock script matches its map.tsv.
    private func bestKey(for args: [String]) -> String? {
        let line = args.joined(separator: " ")
        return outputs.keys.filter { line.hasPrefix($0) }.max { $0.count < $1.count }
    }

    /// The feed of a long-lived command. Tests push lines into it.
    public final class Feed: @unchecked Sendable {
        private let lock = NSLock()
        private var continuations: [AsyncThrowingStream<String, Error>.Continuation] = []
        private var backlog: [String] = []
        public private(set) var opened = 0

        func attach(_ continuation: AsyncThrowingStream<String, Error>.Continuation) {
            lock.lock()
            opened += 1
            continuations.append(continuation)
            let lines = backlog
            backlog = []
            lock.unlock()
            for line in lines { continuation.yield(line) }
        }

        /// Sends a line to every open stream, or keeps it for the next one.
        public func send(_ line: String) {
            lock.lock()
            let open = continuations
            if open.isEmpty { backlog.append(line) }
            lock.unlock()
            for continuation in open { continuation.yield(line) }
        }

        /// Ends every open stream, as when the paddock process dies.
        public func kill() {
            lock.lock()
            let open = continuations
            continuations = []
            lock.unlock()
            for continuation in open { continuation.finish(throwing: CommandError.failed(status: 1, line: "")) }
        }

        /// Ends every open stream normally.
        public func finish() {
            lock.lock()
            let open = continuations
            continuations = []
            lock.unlock()
            for continuation in open { continuation.finish() }
        }

        public var isOpen: Bool {
            lock.lock()
            defer { lock.unlock() }
            return !continuations.isEmpty
        }
    }

    /// The feed for a long-lived command, made on first use.
    public func feed(_ args: [String]) -> Feed {
        lock.lock()
        defer { lock.unlock() }
        let key = args.joined(separator: " ")
        if let feed = feeds[key] { return feed }
        let feed = Feed()
        feeds[key] = feed
        return feed
    }

    public func lines(_ args: [String]) -> AsyncThrowingStream<String, Error> {
        lock.lock()
        _calls.append(Call(time: clock.now(), args: args))
        lock.unlock()
        let feed = self.feed(args)
        return AsyncThrowingStream { continuation in
            feed.attach(continuation)
        }
    }
}
