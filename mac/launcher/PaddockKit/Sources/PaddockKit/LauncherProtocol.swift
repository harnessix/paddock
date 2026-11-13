import Foundation

/// The check of `paddock --launcher-protocol`, the query with which the
/// launcher identifies the paddock that it talks to.
///
/// paddock answers with one JSON object on one line and exit status 0:
/// `{"protocol":"paddock.mac-launcher/v1","features":[...]}`. paddock 0.2.2
/// and older do not know the option: they exit with status 2. The launcher
/// stays unusable until `--version` and this check both pass.
///
/// The answer is a declaration, not a proof: a custom binary can print it.
/// Each JSON output is still checked when it is read.
public enum LauncherProtocol {
    /// The protocol that this launcher speaks.
    public static let name = "paddock.mac-launcher/v1"

    /// The features that this launcher needs. paddock can list more; the
    /// launcher ignores the ones it does not know.
    public static let required: [String] = [
        "app-list-v1",
        "approvals-v1",
        "events-v1",
        "macvm-status-v1",
        "notices-v1",
        "setup-progress-v1",
    ]

    /// The features that this launcher uses when paddock lists them, and
    /// does without on an older paddock: the data of the motions and the
    /// file screens.
    public static let optional: [String] = [
        "backup-files-v1",
        "board-loft-v1",
        "device-call-v1",
        "mail-unread-ask-v1",
        "merge-preview-v1",
        "restore-plan-v1",
    ]

    /// The limits of the query and of `--version`. paddock answers both at
    /// once; the time also covers the first start of a new binary, which
    /// macOS scans.
    public static let limits = RunLimits(seconds: 15, maxBytes: 16 * 1024)

    /// Why a paddock is refused.
    public enum Failure: Equatable, Sendable {
        /// paddock exited with an error: a paddock without the query.
        case noAnswer(status: Int32)
        /// The output is not one JSON object with a string `protocol` and
        /// a list of unique strings `features`.
        case badAnswer
        /// paddock speaks another protocol, maybe a newer one.
        case otherProtocol(String)
        /// paddock lacks features that this launcher needs, in sorted order.
        case missingFeatures([String])
        /// paddock did not answer within the time limit.
        case timedOut
        /// The output was longer than the limit.
        case tooLarge
        /// paddock did not start.
        case notRun
    }

    /// The failure of an answer, or nil when it is good.
    public static func check(_ output: CommandOutput) -> Failure? {
        guard output.status == 0 else { return .noAnswer(status: output.status) }
        guard output.stdout.count <= limits.maxBytes else { return .tooLarge }
        guard case .object(let fields)? = try? StrictJSON.parse(output.stdout),
              case .string(let proto)? = fields["protocol"],
              case .array(let list)? = fields["features"] else { return .badAnswer }
        var features = Set<String>()
        for item in list {
            guard case .string(let feature) = item, features.insert(feature).inserted else { return .badAnswer }
        }
        guard proto == name else { return .otherProtocol(proto) }
        let missing = required.filter { !features.contains($0) }.sorted()
        return missing.isEmpty ? nil : .missingFeatures(missing)
    }

    /// The failure of an error of the bounded run.
    public static func failure(of error: Error) -> Failure {
        switch error as? CommandError {
        case .timedOut?: return .timedOut
        case .tooLarge?: return .tooLarge
        case .failed(let status, _)?: return .noAnswer(status: status)
        case .badJSON?: return .badAnswer
        case .notFound?, nil: return .notRun
        }
    }
}

/// A JSON value of `StrictJSON`.
public indirect enum JSONValue: Equatable, Sendable {
    case object([String: JSONValue])
    case array([JSONValue])
    case string(String)
    case number(String)
    case bool(Bool)
    case null
}

/// A JSON reader that refuses what `JSONSerialization` lets pass: a key
/// twice in one object, text after the value, invalid UTF-8, a deep nest.
/// Whitespace around the value is allowed, so is the final newline.
public enum StrictJSON {
    public struct Invalid: Error, Equatable {
        public var offset: Int
    }

    public static let maxDepth = 32

    public static func parse(_ data: Data) throws -> JSONValue {
        var reader = Reader(bytes: [UInt8](data))
        reader.skipSpace()
        let value = try reader.value(depth: 0)
        reader.skipSpace()
        guard reader.atEnd else { throw Invalid(offset: reader.index) }
        return value
    }

    struct Reader {
        let bytes: [UInt8]
        var index = 0

        var atEnd: Bool { index >= bytes.count }

        func fail() -> Invalid { Invalid(offset: index) }

        mutating func skipSpace() {
            while index < bytes.count, [0x20, 0x09, 0x0A, 0x0D].contains(bytes[index]) { index += 1 }
        }

        mutating func expect(_ byte: UInt8) throws {
            guard index < bytes.count, bytes[index] == byte else { throw fail() }
            index += 1
        }

        mutating func literal(_ word: String) throws {
            for byte in word.utf8 { try expect(byte) }
        }

        mutating func value(depth: Int) throws -> JSONValue {
            guard depth < StrictJSON.maxDepth, index < bytes.count else { throw fail() }
            switch bytes[index] {
            case UInt8(ascii: "{"): return try object(depth: depth)
            case UInt8(ascii: "["): return try array(depth: depth)
            case UInt8(ascii: "\""): return .string(try string())
            case UInt8(ascii: "t"): try literal("true"); return .bool(true)
            case UInt8(ascii: "f"): try literal("false"); return .bool(false)
            case UInt8(ascii: "n"): try literal("null"); return .null
            default: return .number(try number())
            }
        }

        mutating func object(depth: Int) throws -> JSONValue {
            try expect(UInt8(ascii: "{"))
            var fields: [String: JSONValue] = [:]
            skipSpace()
            if index < bytes.count, bytes[index] == UInt8(ascii: "}") {
                index += 1
                return .object(fields)
            }
            while true {
                skipSpace()
                let key = try string()
                guard fields[key] == nil else { throw fail() }
                skipSpace()
                try expect(UInt8(ascii: ":"))
                skipSpace()
                fields[key] = try value(depth: depth + 1)
                skipSpace()
                guard index < bytes.count else { throw fail() }
                if bytes[index] == UInt8(ascii: ",") { index += 1; continue }
                try expect(UInt8(ascii: "}"))
                return .object(fields)
            }
        }

        mutating func array(depth: Int) throws -> JSONValue {
            try expect(UInt8(ascii: "["))
            var items: [JSONValue] = []
            skipSpace()
            if index < bytes.count, bytes[index] == UInt8(ascii: "]") {
                index += 1
                return .array(items)
            }
            while true {
                skipSpace()
                items.append(try value(depth: depth + 1))
                skipSpace()
                guard index < bytes.count else { throw fail() }
                if bytes[index] == UInt8(ascii: ",") { index += 1; continue }
                try expect(UInt8(ascii: "]"))
                return .array(items)
            }
        }

        mutating func string() throws -> String {
            try expect(UInt8(ascii: "\""))
            var out: [UInt8] = []
            while true {
                guard index < bytes.count else { throw fail() }
                let byte = bytes[index]
                index += 1
                switch byte {
                case UInt8(ascii: "\""):
                    // Invalid UTF-8 inside a string is refused, not replaced.
                    guard let text = String(bytes: out, encoding: .utf8) else { throw fail() }
                    return text
                case UInt8(ascii: "\\"):
                    try escape(into: &out)
                case 0x00..<0x20:
                    throw fail()
                default:
                    out.append(byte)
                }
            }
        }

        mutating func escape(into out: inout [UInt8]) throws {
            guard index < bytes.count else { throw fail() }
            let byte = bytes[index]
            index += 1
            let simple: [UInt8: UInt8] = [
                UInt8(ascii: "\""): 0x22, UInt8(ascii: "\\"): 0x5C, UInt8(ascii: "/"): 0x2F,
                UInt8(ascii: "b"): 0x08, UInt8(ascii: "f"): 0x0C, UInt8(ascii: "n"): 0x0A,
                UInt8(ascii: "r"): 0x0D, UInt8(ascii: "t"): 0x09,
            ]
            if let plain = simple[byte] {
                out.append(plain)
                return
            }
            guard byte == UInt8(ascii: "u") else { throw fail() }
            var scalar = try hex4()
            if (0xD800...0xDBFF).contains(scalar) {
                try literal("\\u")
                let low = try hex4()
                guard (0xDC00...0xDFFF).contains(low) else { throw fail() }
                scalar = 0x10000 + ((scalar - 0xD800) << 10) + (low - 0xDC00)
            } else if (0xDC00...0xDFFF).contains(scalar) {
                throw fail()
            }
            guard let unicode = Unicode.Scalar(scalar) else { throw fail() }
            out.append(contentsOf: Array(String(Character(unicode)).utf8))
        }

        mutating func hex4() throws -> UInt32 {
            guard index + 4 <= bytes.count,
                  let text = String(bytes: bytes[index..<index + 4], encoding: .utf8),
                  text.allSatisfy(\.isHexDigit),
                  let value = UInt32(text, radix: 16) else { throw fail() }
            index += 4
            return value
        }

        /// Moves over decimal digits and returns their count.
        mutating func digits() -> Int {
            let from = index
            while index < bytes.count, (0x30...0x39).contains(bytes[index]) { index += 1 }
            return index - from
        }

        /// `-? (0 | [1-9][0-9]*) (. [0-9]+)? ([eE] [+-]? [0-9]+)?`
        mutating func number() throws -> String {
            let start = index
            if index < bytes.count, bytes[index] == UInt8(ascii: "-") { index += 1 }
            guard index < bytes.count else { throw fail() }
            if bytes[index] == UInt8(ascii: "0") {
                index += 1
            } else if digits() == 0 {
                throw fail()
            }
            if index < bytes.count, bytes[index] == UInt8(ascii: ".") {
                index += 1
                guard digits() > 0 else { throw fail() }
            }
            if index < bytes.count, bytes[index] == UInt8(ascii: "e") || bytes[index] == UInt8(ascii: "E") {
                index += 1
                if index < bytes.count, bytes[index] == UInt8(ascii: "+") || bytes[index] == UInt8(ascii: "-") { index += 1 }
                guard digits() > 0 else { throw fail() }
            }
            return String(decoding: bytes[start..<index], as: UTF8.self)
        }
    }
}
