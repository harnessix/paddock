import Foundation
#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif

/// The output of one paddock run.
public struct CommandOutput: Equatable, Sendable {
    public var status: Int32
    public var stdout: Data
    public var stderr: String

    public init(status: Int32, stdout: Data, stderr: String) {
        self.status = status
        self.stdout = stdout
        self.stderr = stderr
    }

    public var stdoutText: String { String(decoding: stdout, as: UTF8.self) }
}

public enum CommandError: Error, Equatable, Sendable {
    /// paddock exited with an error. `line` is its last stderr line.
    case failed(status: Int32, line: String)
    /// The JSON did not decode. `output` is the first 200 characters.
    case badJSON(output: String)
    /// The paddock binary is missing.
    case notFound
    /// A bounded run passed its time limit. The child was killed.
    case timedOut
    /// A bounded run printed more than its limit. The child was killed.
    case tooLarge
}

/// The limits of a bounded run: the time and the bytes on each of stdout
/// and stderr.
public struct RunLimits: Equatable, Sendable {
    public var seconds: Double
    public var maxBytes: Int

    public init(seconds: Double, maxBytes: Int) {
        self.seconds = seconds
        self.maxBytes = maxBytes
    }
}

/// Runs paddock. The only way the launcher reaches paddock.
public protocol CommandRunner: AnyObject, Sendable {
    /// Runs paddock with the arguments and waits for it to exit.
    func run(_ args: [String]) async throws -> CommandOutput
    /// Runs paddock with a time limit and an output limit. Past a limit, or
    /// when the calling task is cancelled, the child is killed and the run
    /// throws `timedOut`, `tooLarge` or `CancellationError`.
    func run(_ args: [String], limits: RunLimits) async throws -> CommandOutput
    /// Starts a long-lived paddock and yields its stdout line by line. The
    /// stream ends when the process exits. Cancelling the consuming task
    /// stops the process with SIGINT.
    func lines(_ args: [String]) -> AsyncThrowingStream<String, Error>
}

/// Runs the paddock binary as a child process.
public final class ProcessRunner: CommandRunner, @unchecked Sendable {
    public let executable: String
    private let lock = NSLock()
    private var children: [Process] = []

    public init(executable: String) {
        self.executable = executable
    }

    public func run(_ args: [String]) async throws -> CommandOutput {
        let executable = self.executable
        return try await withCheckedThrowingContinuation { continuation in
            let process = Process()
            process.executableURL = URL(fileURLWithPath: executable)
            process.arguments = args
            let out = Pipe()
            let err = Pipe()
            process.standardOutput = out
            process.standardError = err
            process.standardInput = FileHandle.nullDevice
            // Read both pipes on their own threads so a large output does not
            // fill the pipe buffer and block paddock.
            let group = DispatchGroup()
            let outBox = DataBox()
            let errBox = DataBox()
            group.enter()
            DispatchQueue.global().async {
                outBox.set(out.fileHandleForReading.readDataToEndOfFile())
                group.leave()
            }
            group.enter()
            DispatchQueue.global().async {
                errBox.set(err.fileHandleForReading.readDataToEndOfFile())
                group.leave()
            }
            process.terminationHandler = { proc in
                let status = proc.terminationStatus
                group.notify(queue: .global()) {
                    continuation.resume(returning: CommandOutput(
                        status: status, stdout: outBox.get(),
                        stderr: String(decoding: errBox.get(), as: UTF8.self)))
                }
            }
            do {
                try process.run()
            } catch {
                continuation.resume(throwing: CommandError.notFound)
            }
        }
    }

    public func run(_ args: [String], limits: RunLimits) async throws -> CommandOutput {
        let job = BoundedJob(executable: executable, args: args, limits: limits)
        return try await withTaskCancellationHandler {
            try await job.run()
        } onCancel: {
            job.stop(.cancelled)
        }
    }

    public func lines(_ args: [String]) -> AsyncThrowingStream<String, Error> {
        let executable = self.executable
        return AsyncThrowingStream { continuation in
            let process = Process()
            process.executableURL = URL(fileURLWithPath: executable)
            process.arguments = args
            let out = Pipe()
            process.standardOutput = out
            process.standardError = FileHandle.nullDevice
            process.standardInput = FileHandle.nullDevice
            let buffer = LineBuffer()
            out.fileHandleForReading.readabilityHandler = { handle in
                let data = handle.availableData
                if data.isEmpty {
                    handle.readabilityHandler = nil
                    return
                }
                for line in buffer.append(data) { continuation.yield(line) }
            }
            process.terminationHandler = { proc in
                out.fileHandleForReading.readabilityHandler = nil
                if let rest = buffer.flush() { continuation.yield(rest) }
                if proc.terminationStatus == 0 || proc.terminationReason == .uncaughtSignal {
                    continuation.finish()
                } else {
                    continuation.finish(throwing: CommandError.failed(status: proc.terminationStatus, line: ""))
                }
            }
            continuation.onTermination = { _ in
                if process.isRunning { process.interrupt() }
            }
            do {
                try process.run()
                self.track(process)
            } catch {
                continuation.finish(throwing: CommandError.notFound)
            }
        }
    }

    private func track(_ process: Process) {
        lock.lock()
        children.removeAll { !$0.isRunning }
        children.append(process)
        lock.unlock()
    }
}

/// One bounded run of `ProcessRunner`. The run ends once: when the child
/// exited and both pipes are at their end, or when a limit or a cancel
/// killed the child and it exited. After a kill the run does not wait for
/// the pipes: a grandchild that holds them open cannot hold the launcher.
///
/// Limit: the kill goes to the child's pid. Between the reap of the child
/// and its termination handler, the pid could in theory be reused.
final class BoundedJob: @unchecked Sendable {
    enum Stop {
        case timedOut, tooLarge, cancelled
    }

    let executable: String
    let args: [String]
    let limits: RunLimits
    private let lock = NSLock()
    private var continuation: CheckedContinuation<CommandOutput, Error>?
    private var process: Process?
    private var started = false
    private var exited = false
    private var status: Int32 = 0
    private var openPipes = 2
    private var stopped: Stop?
    private var ended = false
    private var out = Data()
    private var err = Data()

    init(executable: String, args: [String], limits: RunLimits) {
        self.executable = executable
        self.args = args
        self.limits = limits
    }

    func run() async throws -> CommandOutput {
        try await withCheckedThrowingContinuation { continuation in
            lock.lock()
            self.continuation = continuation
            // A cancel before the start: the child never starts.
            if stopped != nil {
                lock.unlock()
                end()
                return
            }
            let process = Process()
            process.executableURL = URL(fileURLWithPath: executable)
            process.arguments = args
            let outPipe = Pipe()
            let errPipe = Pipe()
            process.standardOutput = outPipe
            process.standardError = errPipe
            process.standardInput = FileHandle.nullDevice
            process.terminationHandler = { [self] proc in
                lock.lock()
                exited = true
                status = proc.terminationStatus
                lock.unlock()
                end()
            }
            self.process = process
            do {
                try process.run()
                started = true
            } catch {
                ended = true
                self.continuation = nil
                lock.unlock()
                continuation.resume(throwing: CommandError.notFound)
                return
            }
            lock.unlock()
            read(outPipe.fileHandleForReading, stdout: true)
            read(errPipe.fileHandleForReading, stdout: false)
            DispatchQueue.global().asyncAfter(deadline: .now() + limits.seconds) { [self] in
                stop(.timedOut)
            }
        }
    }

    /// Reads a pipe on its own thread, so a full pipe never blocks paddock.
    private func read(_ handle: FileHandle, stdout: Bool) {
        let max = limits.maxBytes
        DispatchQueue.global().async { [self] in
            while true {
                let chunk = handle.availableData
                if chunk.isEmpty { break }
                lock.lock()
                if stdout { out.append(chunk) } else { err.append(chunk) }
                let over = (stdout ? out.count : err.count) > max
                let done = stopped != nil
                lock.unlock()
                if over {
                    stop(.tooLarge)
                    break
                }
                if done { break }
            }
            lock.lock()
            openPipes -= 1
            lock.unlock()
            end()
        }
    }

    /// Kills the child for a limit or a cancel. The first reason wins; a
    /// child that already exited is not signalled.
    func stop(_ reason: Stop) {
        lock.lock()
        guard stopped == nil, !ended else {
            lock.unlock()
            return
        }
        stopped = reason
        let pid = started && !exited ? process?.processIdentifier : nil
        lock.unlock()
        if let pid, pid > 0 { kill(pid, SIGKILL) }
        end()
    }

    /// Resumes the caller once the run is over.
    private func end() {
        lock.lock()
        guard !ended, let continuation = self.continuation else {
            lock.unlock()
            return
        }
        let result: Result<CommandOutput, Error>
        switch stopped {
        case .some(let reason):
            // Wait for the exit of a started child, not for its pipes.
            guard !started || exited else {
                lock.unlock()
                return
            }
            switch reason {
            case .timedOut: result = .failure(CommandError.timedOut)
            case .tooLarge: result = .failure(CommandError.tooLarge)
            case .cancelled: result = .failure(CancellationError())
            }
        case .none:
            guard exited, openPipes == 0 else {
                lock.unlock()
                return
            }
            result = .success(CommandOutput(status: status, stdout: out,
                                            stderr: String(decoding: err, as: UTF8.self)))
        }
        ended = true
        self.continuation = nil
        lock.unlock()
        continuation.resume(with: result)
    }
}

/// Splits bytes into lines. A partial last line waits for the next chunk.
final class LineBuffer: @unchecked Sendable {
    private var pending = Data()
    private let lock = NSLock()

    func append(_ data: Data) -> [String] {
        lock.lock()
        defer { lock.unlock() }
        pending.append(data)
        var lines: [String] = []
        while let newline = pending.firstIndex(of: 0x0A) {
            let lineData = pending[pending.startIndex..<newline]
            lines.append(String(decoding: lineData, as: UTF8.self))
            pending = Data(pending[pending.index(after: newline)...])
        }
        return lines
    }

    func flush() -> String? {
        lock.lock()
        defer { lock.unlock() }
        guard !pending.isEmpty else { return nil }
        let line = String(decoding: pending, as: UTF8.self)
        pending = Data()
        return line
    }
}

/// Holds bytes that a reader thread fills.
final class DataBox: @unchecked Sendable {
    private var data = Data()
    private let lock = NSLock()

    func set(_ value: Data) {
        lock.lock()
        data = value
        lock.unlock()
    }

    func get() -> Data {
        lock.lock()
        defer { lock.unlock() }
        return data
    }
}
