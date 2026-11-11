import Foundation

/// The registrable site of a host, the same way paddock's panel finds it
/// (src/macvm/firewall/site.rs): the last two labels, or the last three
/// under a known two-level suffix such as `co.uk` or `github.io`. An IP
/// address or a bad name has no site.
///
/// The question panel offers `Allow <site>` with this value. The launcher
/// shows the same button, so the two must agree: NetworkTests runs the
/// table of the Rust test.
public enum NetSite {
    static let twoLevel: Set<String> = [
        "co.uk", "org.uk", "ac.uk", "gov.uk", "com.au", "net.au", "org.au",
        "co.jp", "ne.jp", "or.jp", "com.cn", "net.cn", "org.cn", "gov.cn",
        "edu.cn", "com.hk", "com.tw", "co.kr", "co.nz", "com.br", "com.mx",
        "co.in", "co.za", "com.sg", "github.io", "vercel.app", "netlify.app",
        "pages.dev", "workers.dev", "herokuapp.com", "appspot.com",
        "cloudfront.net", "amazonaws.com", "azurewebsites.net", "web.app",
        "firebaseapp.com",
    ]

    public static func registrable(_ host: String) -> String? {
        let name = normalized(host)
        let bare = name.trimmingCharacters(in: CharacterSet(charactersIn: "[]"))
        if isAddress(bare) || !validName(name) { return nil }
        let labels = name.split(separator: ".", omittingEmptySubsequences: false).map(String.init)
        guard labels.count >= 2 else { return nil }
        let suffix = labels.suffix(2).joined(separator: ".")
        let count = twoLevel.contains(suffix) ? 3 : 2
        guard labels.count >= count else { return nil }
        return labels.suffix(count).joined(separator: ".")
    }

    /// Lower case, no white space, no trailing dots.
    static func normalized(_ host: String) -> String {
        var name = host.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        while name.hasSuffix(".") { name.removeLast() }
        return name
    }

    /// A host name as paddock accepts it: labels of letters, digits, `-`
    /// and `_`, 1 to 63 characters, no `-` at either end, 253 in all.
    static func validName(_ name: String) -> Bool {
        guard !name.isEmpty, name.count <= 253 else { return false }
        let allowed = Set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_")
        for label in name.split(separator: ".", omittingEmptySubsequences: false) {
            guard !label.isEmpty, label.count <= 63, !label.hasPrefix("-"), !label.hasSuffix("-"),
                  label.allSatisfy({ allowed.contains($0) }) else { return false }
        }
        return true
    }

    /// An IPv4 address in dotted form, or an IPv6 address.
    static func isAddress(_ text: String) -> Bool {
        if text.contains(":") {
            let hex = Set("0123456789abcdefABCDEF:.")
            return text.count >= 2 && text.allSatisfy { hex.contains($0) }
                && text.components(separatedBy: "::").count <= 2
        }
        let parts = text.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 4 else { return false }
        return parts.allSatisfy { part in
            guard !part.isEmpty, part.count <= 3, part.allSatisfy(\.isNumber),
                  let value = Int(part), value <= 255 else { return false }
            return part.count == 1 || part.first != "0"
        }
    }
}
