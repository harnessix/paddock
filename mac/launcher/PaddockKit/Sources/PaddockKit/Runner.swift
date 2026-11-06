import Foundation

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
}

/// Runs paddock. The only way the launcher reaches paddock.
public protocol CommandRunner: AnyObject, Sendable {
    /// Runs paddock with the arguments and waits for it to exit.
    func run(_ args: [String]) async throws -> CommandOutput
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
