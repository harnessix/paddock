import Foundation

/// The texts of the library, sessions, setup and storage that carry
/// values or that PaddockKit picks. Each is a key of Localizable.xcstrings.
public enum LibraryCopy {
    public static let offline = UIText("paddock is not available. Install or update it, then click Check Again.")
    public static let setupStopped = UIText("You stopped the guided step. Try Again continues where it stopped.")
    public static let buildStopped = UIText("The build stopped before the base image was sealed. Try Again continues where it stopped.")
    public static let pastDownload = UIText("The download is done. The later steps cannot pause.")
    public static let notAnApp = UIText("Only apps can be added. Drag an app from your Applications folder.")
    public static let dropHint = UIText("Or drag an app here, or onto paddock for Mac in the Dock.")

    /// `Sample Notes removed. 3.2 GB freed.` plus the trash part when the data
    /// went to paddock's trash.
    public static func removed(_ title: String, freed: Int64, trashBytes: Int64) -> UIText {
        if trashBytes > 0 {
            return UIText("%@ removed. %@ freed. Its data (%@) stays in the paddock trash for 7 days.",
                          title, Format.gb(freed), Format.gb(trashBytes))
        }
        return UIText("%@ removed. %@ freed.", title, Format.gb(freed))
    }

    /// The body of the remove sheet, from what paddock knows of the app.
    public static func removeBody(_ item: AppItem) -> UIText {
        if let sizes = item.json.sizes {
            return UIText("This removes the sandbox of the app (%@ with its saved state %@) and its launcher in your Applications folder. Its data (%@) goes to the paddock trash for 7 days. The original app stays.",
                          Format.gb(sizes.appCopy + sizes.savedState), Format.gb(sizes.savedState), Format.gb(sizes.data))
        }
        return UIText("This removes the sandbox of the app (%@) and its launcher in your Applications folder. Its data goes to the paddock trash for 7 days. The original app stays.",
                      Format.gb(item.json.bytes ?? 0))
    }

    public static func removeTitle(_ item: AppItem, instances: Int) -> UIText {
        instances > 1 ? UIText("Remove %@ and its %@ instances from paddock?", item.instance.title(displayName: item.json.displayName), "\(instances)")
            : UIText("Remove %@ from paddock?", item.title)
    }

    public static func queue(_ index: Int, of count: Int) -> UIText {
        UIText("App %@ of %@", "\(index)", "\(count)")
    }

    public static func existing(_ name: String) -> UIText {
        UIText("This app is in paddock already, as %@.", name)
    }

    public static func versions(launcher: String, paddock: String?) -> UIText {
        if let paddock { return UIText("paddock for Mac %@ · paddock %@", launcher, paddock) }
        return UIText("paddock for Mac %@", launcher)
    }
}
