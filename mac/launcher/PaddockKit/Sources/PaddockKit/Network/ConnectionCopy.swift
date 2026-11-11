import Foundation

/// The filter of the connections list.
public enum ConnectionFilter: String, CaseIterable, Equatable, Sendable, Identifiable {
    case all, denied, allowed

    public var id: String { rawValue }

    public var title: UIText {
        switch self {
        case .all: return UIText("All")
        case .denied: return UIText("Denied")
        case .allowed: return UIText("Allowed")
        }
    }

    public func matches(_ connection: ConnectionJSON) -> Bool {
        switch self {
        case .all: return true
        case .denied: return connection.verdict == "deny"
        case .allowed: return connection.verdict == "allow"
        }
    }
}

/// The texts of a connection row.
public enum ConnectionCopy {
    public static func verdict(_ connection: ConnectionJSON) -> UIText {
        switch connection.verdict {
        case "allow": return UIText("Allowed")
        case "deny": return UIText("Denied")
        default: return UIText("Asked")
        }
    }

    /// The SF Symbol of the verdict. The word always goes with it.
    public static func symbol(_ connection: ConnectionJSON) -> String {
        switch connection.verdict {
        case "allow": return "checkmark.circle"
        case "deny": return "nosign"
        default: return "questionmark.circle"
        }
    }

    public static func times(_ connection: ConnectionJSON) -> UIText {
        let count = connection.count ?? 1
        return count == 1 ? UIText("1 time") : UIText("%@ times", "\(count)")
    }

    /// `api.example.com:443`, or `[::1]:443` for an IPv6 address.
    public static func target(_ connection: ConnectionJSON) -> String {
        let host = connection.host.contains(":") ? "[" + connection.host + "]" : connection.host
        return host + ":" + "\(connection.port)"
    }

    /// Why paddock decided, for the rules that are no rule id.
    public static func reason(_ connection: ConnectionJSON) -> UIText? {
        switch connection.rule {
        case "timeout"?: return UIText("No answer in time")
        case "mode"?: return UIText("The network mode does not allow it")
        case "once"?: return UIText("Allowed once")
        case "ask"?: return UIText("Waiting for an answer")
        default: return nil
        }
    }
}
