use super::*;
use crate::guest::link::BUNDLE_ID;

/// The handler of each scheme in a property list text: `(scheme, bundle)`.
fn handlers(xml: &str) -> Vec<(String, String)> {
    let compact: String = xml.split_whitespace().collect();
    let mut out = Vec::new();
    for part in compact.split("<dict>").skip(1) {
        // The last match: a nested LSHandlerPreferredVersions dict has a
        // LSHandlerRoleAll of its own first.
        let get = |key: &str| {
            let k = format!("<key>{key}</key><string>");
            part.rfind(&k).map(|i| {
                let rest = &part[i + k.len()..];
                rest[..rest.find("</string>").unwrap()].to_string()
            })
        };
        let scheme = get("LSHandlerURLScheme").or_else(|| get("LSHandlerContentType"));
        if let (Some(s), Some(b)) = (scheme, get("LSHandlerRoleAll")) {
            out.push((s, b));
        }
    }
    out
}

fn balanced(xml: &str) {
    for tag in ["dict", "array", "key", "string", "plist"] {
        assert_eq!(
            xml.matches(&format!("<{tag}>")).count() + xml.matches(&format!("<{tag} ")).count(),
            xml.matches(&format!("</{tag}>")).count(),
            "{tag} in\n{xml}"
        );
    }
}

#[test]
fn the_path_is_the_secure_file_of_the_guest_account() {
    assert_eq!(
        rel_path(),
        "Users/paddock/Library/Preferences/com.apple.LaunchServices/com.apple.launchservices.secure.plist"
    );
}

#[test]
fn a_fresh_file_names_the_helper_for_http_and_https_with_role_all() {
    let x = fresh(BUNDLE_ID);
    assert!(x.starts_with("<?xml version=\"1.0\""));
    assert!(x.contains("<!DOCTYPE plist"));
    balanced(&x);
    assert_eq!(
        handlers(&x),
        vec![
            ("http".to_string(), BUNDLE_ID.to_string()),
            ("https".to_string(), BUNDLE_ID.to_string())
        ]
    );
    let compact: String = x.split_whitespace().collect();
    assert!(compact.contains(
        "<key>LSHandlerPreferredVersions</key><dict><key>LSHandlerRoleAll</key><string>-</string></dict>"
    ));
}

/// The file as macOS writes it (`plutil -convert xml1` of a real one):
/// another scheme, a content type and the old browser.
const EXISTING: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>LSHandlers</key>
	<array>
		<dict>
			<key>LSHandlerContentType</key>
			<string>public.html</string>
			<key>LSHandlerRoleAll</key>
			<string>com.apple.safari</string>
		</dict>
		<dict>
			<key>LSHandlerPreferredVersions</key>
			<dict>
				<key>LSHandlerRoleAll</key>
				<string>-</string>
			</dict>
			<key>LSHandlerRoleAll</key>
			<string>com.apple.safari</string>
			<key>LSHandlerURLScheme</key>
			<string>https</string>
		</dict>
		<dict>
			<key>LSHandlerRoleAll</key>
			<string>com.apple.mail</string>
			<key>LSHandlerURLScheme</key>
			<string>mailto</string>
		</dict>
		<dict>
			<key>LSHandlerRoleAll</key>
			<string>com.apple.safari</string>
			<key>LSHandlerURLScheme</key>
			<string>http</string>
		</dict>
	</array>
	<key>LSHandlersVersion</key>
	<integer>1</integer>
</dict>
</plist>
"#;

#[test]
fn a_merge_replaces_only_the_web_entries() {
    let x = merge(EXISTING, BUNDLE_ID).unwrap();
    balanced(&x);
    let h = handlers(&x);
    assert!(h.contains(&("public.html".into(), "com.apple.safari".into())));
    assert!(h.contains(&("mailto".into(), "com.apple.mail".into())));
    assert!(h.contains(&("http".into(), BUNDLE_ID.into())));
    assert!(h.contains(&("https".into(), BUNDLE_ID.into())));
    assert!(!h
        .iter()
        .any(|(s, b)| s.starts_with("http") && b == "com.apple.safari"));
    assert_eq!(h.len(), 4, "{h:?}");
    assert!(x.contains("<key>LSHandlersVersion</key>\n\t<integer>1</integer>"));
    // A second merge changes nothing more.
    let again = merge(&x, BUNDLE_ID).unwrap();
    assert_eq!(handlers(&again), h);
    balanced(&again);
}

#[test]
fn a_merge_adds_the_array_when_there_is_none() {
    let no_key = "<?xml version=\"1.0\"?>\n<plist version=\"1.0\">\n<dict>\n\t<key>LSHandlersVersion</key>\n\t<integer>1</integer>\n</dict>\n</plist>\n";
    let x = merge(no_key, BUNDLE_ID).unwrap();
    balanced(&x);
    assert_eq!(handlers(&x).len(), 2);
    assert!(x.contains("LSHandlersVersion"));

    let empty_root = "<?xml version=\"1.0\"?>\n<plist version=\"1.0\">\n<dict/>\n</plist>\n";
    let x = merge(empty_root, BUNDLE_ID).unwrap();
    balanced(&x);
    assert_eq!(handlers(&x).len(), 2);

    let empty_array =
        "<plist version=\"1.0\">\n<dict>\n\t<key>LSHandlers</key>\n\t<array/>\n</dict>\n</plist>\n";
    let x = merge(empty_array, BUNDLE_ID).unwrap();
    balanced(&x);
    assert_eq!(handlers(&x).len(), 2);
}

#[test]
fn a_key_named_lshandlers_inside_a_value_is_not_the_root_key() {
    let nested = "<plist version=\"1.0\">\n<dict>\n\t<key>Other</key>\n\t<dict>\n\t\t<key>LSHandlers</key>\n\t\t<string>x</string>\n\t</dict>\n</dict>\n</plist>\n";
    let x = merge(nested, BUNDLE_ID).unwrap();
    balanced(&x);
    assert!(x.contains("<key>LSHandlers</key>\n\t\t<string>x</string>"));
    assert_eq!(handlers(&x).len(), 2);
}

#[test]
fn a_file_that_is_not_a_dict_plist_is_an_error() {
    assert_eq!(
        merge("<plist><array/></plist>", BUNDLE_ID).unwrap_err(),
        text::launch_services_no_dict()
    );
    let bad = "<plist><dict><key>LSHandlers</key><string>x</string></dict></plist>";
    assert_eq!(
        merge(bad, BUNDLE_ID).unwrap_err(),
        text::launch_services_no_array()
    );
}

fn data_with_home() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("Users/paddock/Library/Preferences")).unwrap();
    t
}

#[test]
fn write_makes_the_folder_and_names_what_needs_the_owner() {
    use std::os::unix::fs::PermissionsExt;
    let t = data_with_home();
    let never = |_: &Path| -> Result<String> { panic!("no plutil") };
    let w = write(t.path(), BUNDLE_ID, &never).unwrap();
    assert_eq!(
        w.paths,
        vec![
            "Users/paddock/Library/Preferences/com.apple.LaunchServices".to_string(),
            rel_path()
        ]
    );
    assert!(!w.merged);
    let file = t.path().join(rel_path());
    assert_eq!(handlers(&std::fs::read_to_string(&file).unwrap()).len(), 2);
    let mode = std::fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    // Again: the folder is there, the file is merged.
    let w = write(t.path(), BUNDLE_ID, &never).unwrap();
    assert_eq!(w.paths, vec![rel_path()]);
    assert!(w.merged);
    assert_eq!(handlers(&std::fs::read_to_string(&file).unwrap()).len(), 2);
    assert!(!file.with_extension("plist.paddock").exists());
}

#[test]
fn write_converts_a_binary_file_first() {
    let t = data_with_home();
    let dir = t.path().join("Users/paddock").join(FOLDER);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(FILE), b"bplist00\x01\x02").unwrap();
    let conv = |p: &Path| -> Result<String> {
        assert!(p.ends_with(FILE));
        Ok(EXISTING.to_string())
    };
    let w = write(t.path(), BUNDLE_ID, &conv).unwrap();
    assert!(w.merged);
    let x = std::fs::read_to_string(dir.join(FILE)).unwrap();
    assert!(handlers(&x).contains(&("mailto".into(), "com.apple.mail".into())));
    assert!(handlers(&x).contains(&("https".into(), BUNDLE_ID.into())));
}

#[test]
fn write_needs_the_home_and_keeps_a_file_it_cannot_read() {
    let t = tempfile::tempdir().unwrap();
    let never = |_: &Path| -> Result<String> { panic!() };
    assert!(
        format!("{:#}", write(t.path(), BUNDLE_ID, &never).unwrap_err()).contains("Users/paddock")
    );
    let t = data_with_home();
    let dir = t.path().join("Users/paddock").join(FOLDER);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(FILE), "<plist><array/></plist>").unwrap();
    let e = format!("{:#}", write(t.path(), BUNDLE_ID, &never).unwrap_err());
    assert!(e.contains("cannot add the link helper"), "{e}");
    assert_eq!(
        std::fs::read_to_string(dir.join(FILE)).unwrap(),
        "<plist><array/></plist>"
    );
}

/// The offline link step writes the browser in the same attach and gives
/// the new folder and the file the owner of the home.
#[test]
fn the_offline_link_step_sets_the_browser_owned_by_the_account() {
    use crate::macvm::apfs_fixture::Entry;
    use crate::macvm::offline_fake::{FakeDisk, Store};
    let t = tempfile::tempdir().unwrap();
    let image = t.path().join("disk.img");
    let mut d = FakeDisk::new(&image);
    for p in [
        "Users/paddock",
        "Users/paddock/Library",
        "Users/paddock/Library/Preferences",
    ] {
        d.model.push(Entry {
            path: p.into(),
            dir: true,
            uid: 501,
            gid: 20,
        });
    }
    d.store(&image);
    let mnt = t.path().join("mnt");
    let mut tool = Store {
        d: &mut d,
        image: image.clone(),
    };
    super::super::offline_link::link_disk(&mut tool, &image, &mnt).unwrap();
    assert_eq!(d.calls, ["attach", "detach"], "one attach for both");
    for p in [
        "Users/paddock/Library/Preferences/com.apple.LaunchServices".to_string(),
        rel_path(),
    ] {
        assert_eq!(FakeDisk::owner(&image, &p), (501, 20), "{p}");
    }
}
