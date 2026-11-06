import Foundation

// Texts with values that the views show. Fixed texts without values are
// literals in the views; both are keys of Localizable.xcstrings.

public enum QuestionsCopy {
    public static func pausedBanner(_ paused: PausedJSON) -> UIText {
        UIText("%@ asked %@ times in a minute. paddock paused the questions.", paused.app, "\(paused.count)")
    }

    public static func title(_ q: QuestionJSON) -> UIText {
        UIText("%@ wants to reach %@:%@", q.app, q.host, "\(q.port)")
    }
}

public enum RemoveCopy {
    public static let stopFirst = UIText("Stop the app first.")
}

public enum UninstallCopy {
    public static func apps(_ bytes: Int64) -> UIText {
        UIText("Remove sandboxed apps and their saved states (%@)", Format.gb(bytes))
    }

    public static func golden(_ bytes: Int64) -> UIText {
        UIText("Remove the base image (%@)", Format.gb(bytes))
    }

    public static func data(_ bytes: Int64) -> UIText {
        UIText("Remove app data (%@)", Format.gb(bytes))
    }

    public static func done(_ freed: Int64) -> UIText {
        UIText("Done. %@ freed. You can now drag paddock for Mac to the Trash.", Format.gb(freed))
    }
}

public enum StreamCopy {
    public static let pausedLine = UIText("Live updates paused · Refresh")
}

public enum OverviewCopy {
    public static func memory(_ app: AppJSON) -> UIText? {
        guard let mine = app.memoryBytes, let host = app.hostMemoryBytes else { return nil }
        return UIText("%@ of %@", Format.memoryGB(mine), Format.memoryNumber(host))
    }

    public static func openQuestions(_ count: Int) -> UIText {
        UIText("Open questions: %@", "\(count)")
    }

    public static func rules(_ count: Int) -> UIText {
        UIText("Rules: %@", "\(count)")
    }

    public static func used(_ date: Date?, now: Date) -> UIText {
        guard let date else { return UIText("Not used yet") }
        return UIText("Used %@", Format.age(since: date, now: now).english)
    }

    public static func downloads(_ path: String, home: String) -> UIText {
        UIText("Downloads → %@", Format.tildePath(path, home: home))
    }

    public static func appsUsing(_ count: Int) -> UIText {
        UIText("Apps using it: %@", "\(count)")
    }
}

public enum StorageCopy {
    public static func usage(used: Int64, total: Int64) -> UIText {
        UIText("paddock uses %@ of %@", Format.gb(used), Format.gb(total))
    }
}
