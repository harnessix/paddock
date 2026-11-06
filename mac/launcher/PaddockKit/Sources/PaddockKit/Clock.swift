import Foundation

/// The time source of the stores. Tests use `FakeClock`, so a 30-second
/// rule runs without a 30-second wait.
public protocol LauncherClock: AnyObject, Sendable {
    func now() -> Date
    /// Waits for the given time. A fake clock waits until `advance` passes it.
    func sleep(seconds: Double) async throws
}

public final class SystemClock: LauncherClock, @unchecked Sendable {
    public init() {}
    public func now() -> Date { Date() }
    public func sleep(seconds: Double) async throws {
        try await Task.sleep(nanoseconds: UInt64(max(0, seconds) * 1_000_000_000))
    }
}

public final class FakeClock: LauncherClock, @unchecked Sendable {
    private let lock = NSLock()
    private var current: Date
    private var sleepers: [(until: Date, continuation: CheckedContinuation<Void, Error>)] = []

    public init(start: Date = Date(timeIntervalSince1970: 1_790_000_000)) {
        current = start
    }

    public func now() -> Date {
        lock.lock()
        defer { lock.unlock() }
        return current
    }

    public func sleep(seconds: Double) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            lock.lock()
            let until = current.addingTimeInterval(seconds)
            if seconds <= 0 {
                lock.unlock()
                continuation.resume()
                return
            }
            sleepers.append((until, continuation))
            lock.unlock()
        }
    }

    /// Moves the time forward and wakes every sleeper whose time came.
    public func advance(_ seconds: Double) {
        lock.lock()
        current = current.addingTimeInterval(seconds)
        let now = current
        let due = sleepers.filter { $0.until <= now }
        sleepers.removeAll { $0.until <= now }
        lock.unlock()
        for sleeper in due { sleeper.continuation.resume() }
    }

    public var sleeperCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return sleepers.count
    }
}
