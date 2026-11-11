import XCTest
@testable import PaddockKit

/// The network and privacy part: sites, host input, the rules table, the
/// panel answers, recent connections and the clipboard of a session.
@MainActor
final class NetworkTests: XCTestCase {
    // MARK: Pure logic

    // The table of the Rust test in src/macvm/firewall/site.rs: the
    // launcher's `Allow <site>` must name the site the panel names.
    func testSiteMatchesPaddock() {
        let table: [(String, String?)] = [
            ("accounts.example.org", "example.org"),
            ("example.org", "example.org"),
            ("a.b.co.uk", "b.co.uk"),
            ("x.a.github.io", "a.github.io"),
            ("github.io", nil),
            ("co.uk", nil),
            ("com", nil),
            ("127.0.0.1", nil),
            ("[::1]", nil),
            ("A.Example.COM.", "example.com"),
            ("bad..com", nil),
        ]
        for (host, site) in table {
            XCTAssertEqual(NetSite.registrable(host), site, host)
        }
    }

    func testHostInput() {
        XCTAssertEqual(HostInput.parse("api.example.com").input, .host("api.example.com"))
        XCTAssertEqual(HostInput.parse("  API.Example.com. ").input, .host("api.example.com"))
        XCTAssertEqual(HostInput.parse("*.example.com").input, .wildcard("*.example.com"))
        XCTAssertEqual(HostInput.parse("*").input, .everyHost)
        XCTAssertEqual(HostInput.parse("*.co.uk").input, .tooBroad("*.co.uk"))
        XCTAssertEqual(HostInput.parse("*.com").input, .tooBroad("*.com"))
        XCTAssertEqual(HostInput.parse("10.0.0.1").input, .address("10.0.0.1"))
        XCTAssertEqual(HostInput.parse("").input, .empty)
        XCTAssertEqual(HostInput.parse("not a host").input, .invalid)
        XCTAssertEqual(HostInput.parse("a..b").input, .invalid)
        let url = HostInput.parse("https://user@api.example.com:8443/v1?x=1")
        XCTAssertEqual(url.input, .host("api.example.com"))
        XCTAssertEqual(url.port, 8443)
        XCTAssertEqual(HostInput.parse("[::1]:443").input, .address("::1"))
        XCTAssertEqual(HostInput.parse("example.com:abc").input, .invalid)
    }

    func testRuleRowsJoinSitePairsAndDropExpired() throws {
        let list = try Fixtures.decode(NetRulesJSON.self, "rules-list.json")
        XCTAssertEqual(list.rules.count, 5)
        let now = ISO.date("2026-11-06T10:00:00Z")
        let rows = RuleRow.rows(list.rules, now: now)
        XCTAssertEqual(rows.map(\.host), ["*", "tracker.example.net", "example.com", "sample.example.net"])
        let site = rows[2]
        XCTAssertTrue(site.isSite)
        XCTAssertEqual(site.ids, ["rule-0a0a0a", "rule-0a0a0a-subdomains"])
        XCTAssertEqual(site.port, .any)
        XCTAssertEqual(site.count, 12)
        XCTAssertEqual(site.maker, .questionPanel)
        XCTAssertEqual(NetRuleCopy.host(site).english, "example.com and its subdomains")
        XCTAssertEqual(NetRuleCopy.port(site.port).english, "Any port")
        XCTAssertEqual(rows[3].maker, .profile)
        XCTAssertTrue(rows[3].isProfile)
        XCTAssertEqual(NetRuleCopy.scope(rows[1].scope).english, "Until the app stops")
        XCTAssertEqual(NetRuleCopy.verdict(rows[1].verdict).english, "Deny")
        XCTAssertEqual(NetRuleCopy.timeLeft(rows[0], now: now)?.english, "5 min left")
        XCTAssertEqual(NetRuleCopy.timeLeft(rows[0], now: now.addingTimeInterval(270))?.english,
                       "less than a minute left")
        XCTAssertNil(NetRuleCopy.timeLeft(rows[1], now: now))
        // The temporary rule is gone at its end, without a reload.
        let later = RuleRow.rows(list.rules, now: now.addingTimeInterval(300))
        XCTAssertEqual(later.map(\.host), ["tracker.example.net", "example.com", "sample.example.net"])
        XCTAssertEqual(NetRuleCopy.removeTitle([site]).english, "Remove the rule for example.com and its subdomains?")
        XCTAssertEqual(NetRuleCopy.removeTitle(rows).english, "Remove 4 rules?")
    }

    func testDraftChecks() {
        var draft = RuleDraft()
        draft.host = "*"
        guard case .failure(let all) = draft.check(app: "Sample Notes", running: true) else { return XCTFail() }
        XCTAssertEqual(all.command, "paddock app rules Sample Notes add '*' --yes-all-hosts")

        draft.host = "*.co.uk"
        guard case .failure(let broad) = draft.check(app: "Sample Notes", running: true) else { return XCTFail() }
        XCTAssertTrue(broad.text.english.hasPrefix("*.co.uk covers sites of many owners."), broad.text.english)

        draft.host = "files.example.com"
        draft.port = .other
        draft.otherPort = "70000"
        guard case .failure(let port) = draft.check(app: "Sample Notes", running: true) else { return XCTFail() }
        XCTAssertEqual(port.text.english, "Enter a port from 1 to 65535.")

        draft.otherPort = "8443"
        draft.scope = .session
        guard case .failure(let stopped) = draft.check(app: "Sample Notes", running: false) else { return XCTFail() }
        XCTAssertTrue(stopped.text.english.hasPrefix("The app is not running."))
        XCTAssertEqual(try? draft.check(app: "Sample Notes", running: true).get(),
                       CheckedRule(host: "files.example.com", port: .number(8443), site: false, verdict: .allow, scope: .session))

        // The site toggle sends the site, not the typed host.
        var site = RuleDraft()
        site.host = "www.example.org"
        XCTAssertEqual(site.siteSuggestion, "example.org")
        site.site = true
        XCTAssertEqual(try? site.check(app: "Sample Notes", running: false).get().host, "example.org")
        site.host = "10.0.0.1"
        XCTAssertNil(site.siteSuggestion)
        XCTAssertEqual(try? site.check(app: "Sample Notes", running: false).get().site, false)

        // A pasted URL keeps its port.
        var pasted = RuleDraft()
        pasted.host = "https://api.example.com:8443/v1"
        XCTAssertEqual(try? pasted.check(app: "Sample Notes", running: false).get().port, .number(8443))
    }

    // The launcher's answers are the panel's, in the panel's order.
    func testQuestionChoicesMirrorThePanel() {
        let q = QuestionJSON(id: "q9", app: "Sample Notes", host: "accounts.example.org", port: 443)
        XCTAssertEqual(QuestionChoices.title(q).english, "Sample Notes wants to connect to accounts.example.org")
        XCTAssertEqual(QuestionChoices.detail(q).english, "port 443 · example.org and all its subdomains")
        XCTAssertEqual(QuestionChoices.row(q).map(\.answer), [.deny, .allowOnce, .allowSite])
        XCTAssertEqual(QuestionChoices.row(q).map(\.title.english), ["Deny", "Allow once", "Allow example.org"])
        XCTAssertEqual(QuestionChoices.row(q)[0].label.english, "Deny for accounts.example.org")
        XCTAssertEqual(QuestionChoices.row(q)[2].label.english, "Allow example.org and all its subdomains")
        XCTAssertEqual(QuestionChoices.menu(q).map(\.answer), [.allowSession, .denyAlways, .allowAlways, .allowEverything])
        XCTAssertEqual(QuestionChoices.menu(q).map(\.title.english),
                       ["Allow this session", "Always deny", "Always allow accounts.example.org only",
                        "Allow everything for 10 minutes"])
        XCTAssertFalse(PanelAnswer.allowOnce.makesRule)
        XCTAssertTrue(PanelAnswer.allowSite.makesRule)

        let ip = QuestionJSON(id: "q10", app: "Sample Notes", host: "203.0.113.7", port: 8443)
        XCTAssertEqual(QuestionChoices.row(ip)[2].title.english, "Allow 203.0.113.7")
        XCTAssertEqual(QuestionChoices.row(ip)[2].label.english, "Always allow 203.0.113.7")
        XCTAssertEqual(QuestionChoices.detail(ip).english, "port 8443 · first connection to this host")
        // The flags are the words of paddock's ConnectAnswer.
        XCTAssertEqual(PanelAnswer.allCases.map(\.flag),
                       ["--deny", "--allow-once", "--allow-site", "--allow-session", "--deny-always",
                        "--allow-always", "--allow-everything"])
    }

    func testOpenFilesText() {
        XCTAssertNil(NetworkModel.openFiles(ShareJSON(path: "/a", mode: "ro", openHandles: 0)))
        XCTAssertEqual(NetworkModel.openFiles(ShareJSON(path: "/a", mode: "ro", openHandles: 1))?.english, "1 file open")
        XCTAssertEqual(NetworkModel.openFiles(ShareJSON(path: "/a", mode: "rw", openHandles: 2))?.english, "2 files open")
    }

    // MARK: Questions

    func testAnswerFromTheMoreMenu() async throws {
        let h = try await Harness("questions")
        XCTAssertEqual(h.store.questions.map(\.id), ["q1", "q2"])
        let q = h.store.questions[0]
        let line = await h.store.answer(q, panel: .allowEverything)
        XCTAssertNil(line)
        XCTAssertEqual(h.calls(["approvals", "answer"]).last?.args, ["approvals", "answer", "q1", "--allow-everything"])
        XCTAssertEqual(h.store.questions.map(\.id), ["q2"])

        _ = await h.store.answer(h.store.questions[0], panel: .allowSite)
        XCTAssertEqual(h.calls(["approvals", "answer"]).last?.args, ["approvals", "answer", "q2", "--allow-site"])
    }

    func testFailedAnswerKeepsTheQuestion() async throws {
        let h = try await Harness("questions")
        h.runner.reset(prefix: "approvals answer")
        h.runner.on(prefix: "approvals answer",
                    output: CommandOutput(status: 1, stdout: Data(), stderr: "no open question q1\n"))
        let line = await h.store.answer(h.store.questions[0], panel: .deny)
        XCTAssertEqual(line, "no open question q1")
        XCTAssertEqual(h.store.questions.count, 2)
    }

    // MARK: Rules

    func makeModel(_ scenario: String = "ready") async throws -> (Harness, NetworkModel) {
        let h = try await Harness(scenario)
        let model = NetworkModel(name: "Sample Notes", store: h.store)
        await model.loadRules(force: true)
        return (h, model)
    }

    func testLoadAndAddSiteRule() async throws {
        let (h, model) = try await makeModel()
        XCTAssertEqual(model.rulesState, .loaded)
        XCTAssertEqual(model.rows(now: h.clock.now()).count, 4)
        var draft = RuleDraft()
        draft.host = "www.example.org"
        draft.site = true
        let message = await model.save(draft)
        XCTAssertNil(message)
        XCTAssertEqual(h.calls(["app", "rules", "Sample Notes", "add"]).last?.args,
                       ["app", "rules", "Sample Notes", "add", "example.org", "--port", "443", "--allow", "--site", "--json"])
        let row = model.rows(now: h.clock.now()).first { $0.host == "example.org" }
        XCTAssertEqual(row?.isSite, true)
        XCTAssertEqual(row?.maker, .you)
        let change = h.store.detail(for: "Sample Notes").changes.first
        XCTAssertEqual(change?.text, "Allow always example.org and its subdomains")
        XCTAssertEqual(change?.source, "you")
    }

    func testEditChangesTheRuleInPlace() async throws {
        let (h, model) = try await makeModel()
        let tracker = try XCTUnwrap(model.rows(now: h.clock.now()).first { $0.host == "tracker.example.net" })
        var draft = RuleDraft(row: tracker)
        XCTAssertEqual(draft.verdict, .deny)
        XCTAssertEqual(draft.scope, .session)
        XCTAssertEqual(draft.port, .https)
        draft.scope = .always
        let message = await model.save(draft, editing: tracker)
        XCTAssertNil(message)
        XCTAssertEqual(h.calls(["app", "rules", "Sample Notes", "change"]).last?.args,
                       ["app", "rules", "Sample Notes", "change", "session-7c7c7c", "--host", "tracker.example.net",
                        "--port", "443", "--deny", "--always", "--json"])
        XCTAssertFalse(model.rules.contains { $0.id == "session-7c7c7c" })
        XCTAssertEqual(model.rules.first { $0.host == "tracker.example.net" }?.scope, "always")
    }

    func testRemoveSendsBothIdsOfASiteAndSkipsTheProfile() async throws {
        let (h, model) = try await makeModel()
        let rows = model.rows(now: h.clock.now())
        let site = try XCTUnwrap(rows.first { $0.isSite })
        let seed = try XCTUnwrap(rows.first { $0.isProfile })
        await model.remove([site, seed])
        XCTAssertEqual(h.calls(["app", "rules", "Sample Notes", "remove"]).last?.args,
                       ["app", "rules", "Sample Notes", "remove", "rule-0a0a0a", "rule-0a0a0a-subdomains", "--json"])
        XCTAssertFalse(model.rules.contains { $0.host.hasSuffix("example.com") })
        XCTAssertTrue(model.rules.contains { $0.id == seed.ids[0] })
        // Only profile rules: no call at all.
        let calls = h.calls(["app", "rules", "Sample Notes", "remove"]).count
        await model.remove([seed])
        XCTAssertEqual(h.calls(["app", "rules", "Sample Notes", "remove"]).count, calls)
    }

    func testRefusalIsShownVerbatim() async throws {
        let (_, model) = try await makeModel()
        var draft = RuleDraft()
        draft.host = "10.0.0.1"
        let message = await model.save(draft)
        XCTAssertEqual(message?.text.english,
                       "paddock refuses a rule for 10.0.0.1: the guest may not reach this Mac's local network.")
        XCTAssertFalse(model.rules.contains { $0.host == "10.0.0.1" })
    }

    func testEveryHostNeverReachesPaddock() async throws {
        let (h, model) = try await makeModel()
        var draft = RuleDraft()
        draft.host = "*"
        let message = await model.save(draft)
        XCTAssertEqual(message?.command, ShellCommands.allHostsRule(app: "Sample Notes"))
        XCTAssertTrue(h.calls(["app", "rules", "Sample Notes", "add"]).isEmpty)
    }

    func testOldPaddockAsksForAnUpdate() async throws {
        let h = try await Harness("old-cli")
        let model = NetworkModel(name: "Sample Notes", store: h.store)
        await model.loadRules(force: true)
        XCTAssertEqual(model.rulesState, .needsNewerPaddock)
        await model.loadConnections(force: true)
        XCTAssertEqual(model.connectionsState, .needsNewerPaddock)
        // The old flat clipboard JSON is the profile.
        await model.loadClipboard(force: true)
        XCTAssertEqual(model.clipboard, ClipboardStateJSON(profile: ClipboardJSON(toApp: true, fromApp: true),
                                                           session: nil, running: false))
        // Another failure keeps paddock's line.
        XCTAssertEqual(NetLoad.of(CommandError.failed(status: 1, line: "the session did not answer")),
                       .failed("the session did not answer"))
        XCTAssertEqual(NetLoad.of(CommandError.notFound), .offline)
        // clap ends an argument error with a usage hint: the error line counts.
        XCTAssertEqual(NetLoad.errorLine("error: unexpected argument '--json' found\n\nUsage: paddock app rules <NAME>\n\nFor more information, try '--help'.\n"),
                       "error: unexpected argument '--json' found")
        XCTAssertEqual(NetLoad.errorLine("warning: slow\nthe session did not answer\n"), "the session did not answer")
    }

    func testConnectionTexts() throws {
        let list = try Fixtures.decode(ConnectionsJSON.self, "connections.json")
        let deny = list.connections[0]
        XCTAssertEqual(ConnectionCopy.verdict(deny).english, "Denied")
        XCTAssertEqual(ConnectionCopy.symbol(deny), "nosign")
        XCTAssertEqual(ConnectionCopy.times(deny).english, "5 times")
        XCTAssertEqual(ConnectionCopy.target(deny), "telemetry.example.net:443")
        XCTAssertEqual(deny.seen, ISO.date("2026-11-06T09:59:40Z"))
        let ip = list.connections[2]
        XCTAssertEqual(ConnectionCopy.times(ip).english, "1 time")
        XCTAssertEqual(ConnectionCopy.reason(ip)?.english, "No answer in time")
        XCTAssertEqual(ConnectionCopy.verdict(list.connections[3]).english, "Asked")
        XCTAssertEqual(ConnectionCopy.target(ConnectionJSON(time: Date(), host: "::1", port: 443, verdict: "allow")), "[::1]:443")
        XCTAssertEqual(list.connections.filter(ConnectionFilter.denied.matches).count, 2)
        XCTAssertEqual(list.connections.filter(ConnectionFilter.allowed.matches).count, 1)
        XCTAssertEqual(list.connections.filter(ConnectionFilter.all.matches).count, 4)
    }

    func testOfflineWithoutPaddock() async {
        let store = LauncherStore(service: nil, clock: FakeClock())
        let model = NetworkModel(name: "Sample Notes", store: store)
        await model.loadRules(force: true)
        await model.loadConnections(force: true)
        await model.loadClipboard(force: true)
        XCTAssertEqual(model.rulesState, .offline)
        XCTAssertEqual(model.connectionsState, .offline)
        XCTAssertEqual(model.clipboardState, .offline)
    }

    func testRuleLoadsAreGated() async throws {
        let (h, model) = try await makeModel()
        await model.loadRules()
        XCTAssertEqual(h.calls(["app", "rules", "Sample Notes", "list"]).count, 1)
        h.clock.advance(5)
        await model.loadRules()
        XCTAssertEqual(h.calls(["app", "rules", "Sample Notes", "list"]).count, 2)
    }

    // MARK: Connections

    func testAllowADeniedHostOrItsSite() async throws {
        let (h, model) = try await makeModel()
        await model.loadConnections(force: true)
        XCTAssertEqual(model.connectionsState, .loaded)
        XCTAssertEqual(model.connections.first?.host, "telemetry.example.net")
        XCTAssertEqual(model.connections.filter(\.isDenied).count, 2)
        let telemetry = model.connections[0]

        await model.allow(telemetry, site: false)
        XCTAssertEqual(h.calls(["app", "rules", "Sample Notes", "add"]).last?.args,
                       ["app", "rules", "Sample Notes", "add", "telemetry.example.net", "--port", "443", "--allow", "--json"])
        XCTAssertEqual(model.allowed[telemetry.id]?.english, "Allowed · rule added. The app connects on its next try.")
        XCTAssertTrue(model.rules.contains { $0.host == "telemetry.example.net" && $0.verdict == "allow" })

        await model.allow(telemetry, site: true)
        XCTAssertEqual(h.calls(["app", "rules", "Sample Notes", "add"]).last?.args,
                       ["app", "rules", "Sample Notes", "add", "example.net", "--port", "*", "--allow", "--site", "--json"])
        XCTAssertTrue(model.rows(now: h.clock.now()).contains { $0.isSite && $0.host == "example.net" })
        XCTAssertNil(model.problem)
    }

    func testFollowingConnectionsReloadsEveryTenSeconds() async throws {
        let (h, model) = try await makeModel()
        let base = h.clock.sleeperCount
        let task = Task { await model.followConnections() }
        // Advance only once the loop sleeps, so the wake-up is not lost.
        await waitUntil { h.calls(["app", "connections"]).count == 1 && h.clock.sleeperCount > base }
        h.clock.advance(NetworkModel.connectionsEvery)
        await waitUntil { h.calls(["app", "connections"]).count == 2 }
        task.cancel()
        h.clock.advance(NetworkModel.connectionsEvery)
        await settle()
        XCTAssertEqual(h.calls(["app", "connections"]).count, 2)
    }

    // MARK: Clipboard

    func testClipboardOfTheSessionOnly() async throws {
        let (h, model) = try await makeModel()
        await model.loadClipboard(force: true)
        XCTAssertEqual(model.clipboard?.running, true)
        XCTAssertEqual(model.clipboard?.session, ClipboardJSON(toApp: true, fromApp: false))

        await model.setClipboard(session: ClipboardJSON(toApp: true, fromApp: true))
        XCTAssertEqual(h.calls(["app", "clipboard", "Sample Notes", "--to-app"]).last?.args,
                       ["app", "clipboard", "Sample Notes", "--to-app", "on", "--from-app", "on", "--session", "--json"])
        XCTAssertEqual(model.clipboard?.profile, ClipboardJSON(toApp: true, fromApp: false))
        XCTAssertEqual(model.clipboard?.session, ClipboardJSON(toApp: true, fromApp: true))

        // The profile applies to the running session too.
        await model.setClipboard(profile: ClipboardJSON(toApp: false, fromApp: false))
        XCTAssertEqual(h.calls(["app", "clipboard", "Sample Notes", "--to-app"]).last?.args,
                       ["app", "clipboard", "Sample Notes", "--to-app", "off", "--from-app", "off", "--json"])
        XCTAssertEqual(model.clipboard?.session, ClipboardJSON(toApp: false, fromApp: false))
        XCTAssertEqual(h.store.detail(for: "Sample Notes").changes.first?.text, "Clipboard to app off, from app off")
    }

    func testFailedClipboardChangeRollsBack() async throws {
        let (h, model) = try await makeModel()
        await model.loadClipboard(force: true)
        h.runner.reset(prefix: "app clipboard")
        h.runner.on(prefix: "app clipboard",
                    output: CommandOutput(status: 1, stdout: Data(), stderr: "the session did not answer\n"))
        await model.setClipboard(session: ClipboardJSON(toApp: false, fromApp: false))
        XCTAssertEqual(model.clipboard?.session, ClipboardJSON(toApp: true, fromApp: false))
        XCTAssertEqual(model.problem?.text.english, "the session did not answer")
    }

    // MARK: Folders

    func testSessionOnlyFolderCanBeKept() async throws {
        let (h, model) = try await makeModel()
        let detail = h.store.detail(for: "Sample Notes")
        await detail.load(force: true)
        XCTAssertEqual(detail.shares.first { $0.path == "/Users/demo/Documents/Notes" }?.from, "run")
        await model.keepFolder("/Users/demo/Documents/Notes")
        XCTAssertEqual(h.calls(["app", "share", "Sample Notes", "keep"]).last?.args,
                       ["app", "share", "Sample Notes", "keep", "/Users/demo/Documents/Notes", "--json"])
        XCTAssertEqual(detail.shares.first { $0.path == "/Users/demo/Documents/Notes" }?.from, "grant")
    }
}
