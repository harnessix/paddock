//! The default browser of the guest, set offline, platform-neutral.
//!
//! Launch Services keeps the handler of each URL scheme of a user in
//! `~/Library/Preferences/com.apple.LaunchServices/com.apple.launchservices.secure.plist`,
//! in the array `LSHandlers`. One entry per scheme names the bundle with
//! `LSHandlerRoleAll`. A program that sets the default browser itself
//! (`LSSetDefaultHandlerForURLScheme`) makes macOS ask the person, in a
//! prompt that nobody in the guest answers. With the guest stopped, the
//! file is a plain file of the Data volume, so the offline step of the
//! golden (`offline_link.rs`) writes the entries for `http` and `https`
//! that name the link helper ([`crate::guest::link::BUNDLE_ID`]).
//!
//! The file may exist already, as a binary or an XML property list. A
//! binary one becomes XML first (`plutil -convert xml1` on the Mac). The
//! merge keeps every other entry and replaces only the `http` and `https`
//! entries. The file, and each folder that the write made, then get the
//! owner of the home through the APFS owner patch, as the Downloads link.

use super::apfs;
use super::text;
use anyhow::{anyhow, Context, Result};
use std::path::Path;

/// The schemes that the helper takes.
pub const SCHEMES: [&str; 2] = ["http", "https"];

/// The folder of the file, relative to the guest home.
pub const FOLDER: &str = "Library/Preferences/com.apple.LaunchServices";
/// The file name.
pub const FILE: &str = "com.apple.launchservices.secure.plist";

/// The file relative to the root of the Data volume.
pub fn rel_path() -> String {
    format!("{}/{FOLDER}/{FILE}", super::offline_link::home()) // not user text
}

/// The entry of one scheme, at the indent of an element of `LSHandlers`.
fn entry(scheme: &str, bundle: &str) -> String {
    let e = format!(
        // not user text
        "\t\t<dict>\n\t\t\t<key>LSHandlerPreferredVersions</key>\n\t\t\t<dict>\n\t\t\t\t<key>LSHandlerRoleAll</key>\n\t\t\t\t<string>-</string>\n\t\t\t</dict>\n\t\t\t<key>LSHandlerRoleAll</key>\n\t\t\t<string>{bundle}</string>\n\t\t\t<key>LSHandlerURLScheme</key>\n\t\t\t<string>{scheme}</string>\n\t\t</dict>\n"
    );
    e
}

/// The entries of the helper, one per scheme of [`SCHEMES`].
pub fn entries(bundle: &str) -> String {
    SCHEMES.iter().map(|s| entry(s, bundle)).collect()
}

const HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n";

/// A new file with only the entries of the helper.
pub fn fresh(bundle: &str) -> String {
    let body = format!(
        // not user text
        "{HEAD}<dict>\n\t<key>LSHandlers</key>\n\t<array>\n{}\t</array>\n</dict>\n</plist>\n",
        entries(bundle)
    );
    body
}

/// One tag of an XML property list: its byte range and its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    Open,
    Close,
    Empty,
}

/// The next tag at or after `from` whose name is `name`: its start, its
/// end (after `>`) and its kind. Comments, the declaration and the
/// doctype are skipped.
fn next_tag(s: &str, from: usize, names: &[&str]) -> Option<(usize, usize, Tag, String)> {
    let mut i = from;
    while let Some(off) = s[i..].find('<') {
        let start = i + off;
        if s[start..].starts_with("<!--") {
            let end = s[start..].find("-->")? + start + 3;
            i = end;
            continue;
        }
        let end = s[start..].find('>')? + start + 1;
        let inner = &s[start + 1..end - 1];
        i = end;
        if inner.starts_with('?') || inner.starts_with('!') {
            continue;
        }
        let (kind, name) = if let Some(n) = inner.strip_prefix('/') {
            (Tag::Close, n.trim())
        } else if let Some(n) = inner.strip_suffix('/') {
            (Tag::Empty, n.trim())
        } else {
            (Tag::Open, inner.split_whitespace().next().unwrap_or(""))
        };
        if names.contains(&name) {
            return Some((start, end, kind, name.to_string()));
        }
    }
    None
}

/// The end (after the closing tag) of the element `name` whose opening
/// tag ends at `from`.
fn element_end(s: &str, from: usize, name: &str) -> Option<usize> {
    let mut depth = 1;
    let mut i = from;
    while let Some((_, end, kind, _)) = next_tag(s, i, &[name]) {
        i = end;
        match kind {
            Tag::Open => depth += 1,
            Tag::Close => {
                depth -= 1;
                if depth == 0 {
                    return Some(end);
                }
            }
            Tag::Empty => {}
        }
    }
    None
}

/// Whether an `LSHandlers` element (a `<dict>...</dict>`) is the entry of
/// a scheme of [`SCHEMES`].
fn is_web_entry(dict: &str) -> bool {
    let compact: String = dict.split_whitespace().collect();
    SCHEMES.iter().any(|s| {
        compact.contains(&format!(
            "<key>LSHandlerURLScheme</key><string>{s}</string>"
        )) // not user text
    })
}

/// Merge the entries of the helper into the XML property list `xml`: the
/// entries of `http` and `https` go, every other entry and key stays. An
/// error names what the file lacks.
pub fn merge(xml: &str, bundle: &str) -> Result<String, String> {
    let (_, root_end, kind, _) =
        next_tag(xml, 0, &["dict"]).ok_or_else(text::launch_services_no_dict)?;
    if kind == Tag::Empty {
        // `<dict/>`: an empty root.
        let start = xml
            .find("<dict/>")
            .ok_or_else(text::launch_services_no_dict)?;
        let body = format!(
            // not user text
            "{}<dict>\n\t<key>LSHandlers</key>\n\t<array>\n{}\t</array>\n</dict>{}",
            &xml[..start],
            entries(bundle),
            &xml[start + "<dict/>".len()..]
        );
        return Ok(body);
    }
    if kind != Tag::Open {
        return Err(text::launch_services_no_dict());
    }
    let root_close =
        element_end(xml, root_end, "dict").ok_or_else(text::launch_services_no_dict)?;
    // The keys of the root dict, at depth one: find `LSHandlers`.
    let mut i = root_end;
    let mut handlers = None;
    while let Some((start, end, kind, name)) = next_tag(
        xml,
        i,
        &[
            "key", "dict", "array", "string", "integer", "true", "false", "data", "date", "real",
        ],
    ) {
        if start >= root_close {
            break;
        }
        if name == "key" && kind == Tag::Open {
            let close = xml[end..]
                .find("</key>")
                .map(|o| end + o)
                .ok_or_else(text::launch_services_no_dict)?;
            if xml[end..close].trim() == "LSHandlers" {
                handlers = Some(close + "</key>".len());
                break;
            }
            i = close + "</key>".len();
            continue;
        }
        // A value: skip the whole element.
        i = if kind == Tag::Open {
            element_end(xml, end, &name).ok_or_else(text::launch_services_no_dict)?
        } else {
            end
        };
    }
    let Some(after_key) = handlers else {
        // No LSHandlers yet: the key and the array go first in the root.
        let body = format!(
            // not user text
            "{}\n\t<key>LSHandlers</key>\n\t<array>\n{}\t</array>{}",
            &xml[..root_end],
            entries(bundle),
            &xml[root_end..]
        );
        return Ok(body);
    };
    let (astart, aend, akind, _) =
        next_tag(xml, after_key, &["array"]).ok_or_else(text::launch_services_no_array)?;
    if xml[after_key..astart].trim() != "" {
        return Err(text::launch_services_no_array());
    }
    if akind == Tag::Empty {
        let body = format!(
            // not user text
            "{}<array>\n{}\t</array>{}",
            &xml[..astart],
            entries(bundle),
            &xml[aend..]
        );
        return Ok(body);
    }
    if akind != Tag::Open {
        return Err(text::launch_services_no_array());
    }
    let array_close = element_end(xml, aend, "array").ok_or_else(text::launch_services_no_array)?;
    let inner_end = array_close - "</array>".len();
    // The elements of the array: keep the dicts that are not web entries.
    let mut kept = String::new();
    let mut j = aend;
    while let Some((start, end, kind, _)) = next_tag(xml, j, &["dict"]) {
        if start >= inner_end {
            break;
        }
        let stop = match kind {
            Tag::Open => {
                element_end(xml, end, "dict").ok_or_else(text::launch_services_no_array)?
            }
            _ => end,
        };
        let one = &xml[start..stop];
        if !is_web_entry(one) {
            kept.push_str("\t\t");
            kept.push_str(one);
            kept.push('\n');
        }
        j = stop;
    }
    let body = format!(
        // not user text
        "{}\n{kept}{}\t</array>{}",
        &xml[..aend],
        entries(bundle),
        &xml[array_close..]
    );
    Ok(body)
}

/// A binary property list.
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.starts_with(b"bplist")
}

/// What [`write`] did, for the owner patch: the paths relative to the
/// Data volume root that need the owner of the home, parents first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Written {
    pub paths: Vec<String>,
    /// The file was there before: its other entries were kept.
    pub merged: bool,
}

/// Write the entries of the helper into the file under the mount `data`
/// of the Data volume. `to_xml` turns a binary property list into XML
/// (`plutil` on the Mac).
pub fn write(
    data: &Path,
    bundle: &str,
    to_xml: &dyn Fn(&Path) -> Result<String>,
) -> Result<Written> {
    use std::os::unix::fs::PermissionsExt;
    let home_rel = super::offline_link::home();
    let home = data.join(&home_rel);
    if !home.is_dir() {
        return Err(anyhow!(text::downloads_no_home(&home)));
    }
    let mut out = Written::default();
    // Each folder of the path that is not there yet, parents first.
    let mut rel = home_rel.clone();
    for part in FOLDER.split('/') {
        rel = format!("{rel}/{part}"); // not user text
        let p = data.join(&rel);
        if !p.is_dir() {
            std::fs::create_dir(&p).with_context(|| text::cannot_make(&p))?;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700))
                .with_context(|| text::cannot_write(&p))?;
            out.paths.push(rel.clone());
        }
    }
    let file = data.join(rel_path());
    let body = match std::fs::read(&file) {
        Ok(bytes) if bytes.iter().all(|b| b.is_ascii_whitespace()) => fresh(bundle),
        Ok(bytes) => {
            let xml = if is_binary(&bytes) {
                to_xml(&file)?
            } else {
                String::from_utf8(bytes)
                    .map_err(|_| anyhow!(text::launch_services_unreadable(&file)))?
            };
            out.merged = true;
            merge(&xml, bundle).map_err(|why| anyhow!(text::launch_services_bad(&file, &why)))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fresh(bundle),
        Err(e) => return Err(anyhow::Error::new(e).context(text::cannot_open(&file))),
    };
    let tmp = file.with_extension("plist.paddock");
    std::fs::write(&tmp, body.as_bytes()).with_context(|| text::cannot_write(&tmp))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
        .with_context(|| text::cannot_write(&tmp))?;
    std::fs::rename(&tmp, &file).with_context(|| text::cannot_write(&file))?;
    out.paths.push(rel_path());
    Ok(out)
}

/// `plutil -convert xml1 -o - FILE` on the Mac.
pub fn plutil_xml(file: &Path) -> Result<String> {
    let out = std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "xml1", "-o", "-"])
        .arg(file)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| text::launch_services_unreadable(file))?;
    if !out.status.success() {
        return Err(anyhow!(text::launch_services_unreadable(file)));
    }
    String::from_utf8(out.stdout).map_err(|_| anyhow!(text::launch_services_unreadable(file)))
}

/// Give the paths of `w` the owner of the home on the detached image
/// `disk`, and read them back.
pub fn own(disk: &Path, w: &Written) -> Result<(u32, u32)> {
    let (uid, gid) = super::offline_link::home_owner(disk)?;
    let paths: Vec<&str> = w.paths.iter().map(String::as_str).collect();
    if paths.is_empty() {
        return Ok((uid, gid));
    }
    apfs::chown(disk, apfs::ROLE_DATA, uid, gid, &paths)?;
    for (p, now) in paths
        .iter()
        .zip(apfs::owners(disk, apfs::ROLE_DATA, &paths)?)
    {
        if now != (uid, gid) {
            return Err(anyhow!(text::apfs_unexpected_owner(p, now.0, now.1)));
        }
    }
    Ok((uid, gid))
}

#[cfg(test)]
#[path = "launch_services_tests.rs"]
mod tests;
