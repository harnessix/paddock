use super::*;

fn shares() -> Vec<Share> {
    vec![
        Share {
            name: "ws-1".into(),
            host: PathBuf::from("/Users/me/src/web"),
            read_only: false,
        },
        Share {
            name: "drop".into(),
            host: PathBuf::from("/p/drop"),
            read_only: false,
        },
    ]
}

fn sent(a: &[Action]) -> &Payload {
    match a {
        [Action::SendToGuest(p)] => p,
        other => panic!("{other:?}"),
    }
}

fn set(a: &[Action]) -> &Payload {
    match a {
        [Action::SetHost(p)] => p,
        other => panic!("{other:?}"),
    }
}

/// Item 9 of the security checklist: a host change while not key sends
/// nothing, a window becoming key sends the current host content once, a
/// guest change while key sets the host, a guest change while not key
/// does nothing.
#[test]
fn moves_only_while_key() {
    let mut s = Sync::new(shares());
    assert!(!s.is_key());
    let one = vec![Item::text("one")];
    assert_eq!(s.on_host_change(1, one.clone()), [Action::Nothing]);
    assert_eq!(
        s.on_host_change(2, vec![Item::text("two")]),
        [Action::Nothing]
    );
    let a = s.on_key(true);
    assert_eq!(
        sent(&a).items,
        [Item::text("two")],
        "the current content, once"
    );
    assert_eq!(s.on_key(true), [Action::Nothing]);
    assert_eq!(s.on_key(false), [Action::Nothing]);
    assert_eq!(s.on_key(true), [Action::Nothing], "not sent again");
    // A host change while key goes at once; the same count again does not.
    let a = s.on_host_change(3, vec![Item::text("three")]);
    assert_eq!(sent(&a).items, [Item::text("three")]);
    assert_eq!(
        s.on_host_change(3, vec![Item::text("three")]),
        [Action::Nothing]
    );
    // The guest's echo of what was sent is not a change.
    assert_eq!(
        s.on_guest_change(10, vec![Item::text("three")]),
        [Action::Nothing]
    );
    // A guest change while key sets the host.
    let a = s.on_guest_change(11, vec![Item::text("from guest")]);
    assert_eq!(set(&a).items, [Item::text("from guest")]);
    assert_eq!(
        s.on_guest_change(11, vec![Item::text("from guest")]),
        [Action::Nothing]
    );
    // The host's echo of what was set does not go back to the guest.
    assert_eq!(
        s.on_host_change(4, vec![Item::text("from guest")]),
        [Action::Nothing]
    );
    assert_eq!(s.on_key(false), [Action::Nothing]);
    assert_eq!(s.on_key(true), [Action::Nothing], "the echo counts as sent");
    // Not key: a guest change does nothing, a host change waits for key.
    s.on_key(false);
    assert_eq!(
        s.on_guest_change(12, vec![Item::text("secret of guest")]),
        [Action::Nothing]
    );
    assert_eq!(
        s.on_host_change(5, vec![Item::text("secret of host")]),
        [Action::Nothing]
    );
    assert_eq!(
        s.on_host_change(6, vec![Item::text("later")]),
        [Action::Nothing]
    );
    let a = s.on_key(true);
    assert_eq!(
        sent(&a).items,
        [Item::text("later")],
        "only the latest, on key"
    );
    // The guest change of before key is gone: it is not sent later.
    assert_eq!(
        s.on_guest_change(12, vec![Item::text("secret of guest")]),
        [Action::Nothing]
    );
}

/// The types in the order of `TYPES`, unknown types out, a concealed item
/// out and the rest goes, a transient item out, an auto generated set
/// not re-sent.
#[test]
fn type_order_and_filters() {
    let mut s = Sync::new(shares());
    s.on_key(true);
    let item = Item::default()
        .with("com.apple.webarchive", b"w")
        .with("public.png", b"png")
        .with("public.utf8-plain-text", b"text")
        .with("public.rtf", b"rtf")
        .with("dyn.ah62d4rv4gu8y", b"x");
    let a = s.on_host_change(1, vec![item]);
    let types: Vec<&str> = sent(&a).items[0]
        .types
        .iter()
        .map(|(t, _)| t.as_str())
        .collect();
    assert_eq!(
        types,
        ["public.utf8-plain-text", "public.rtf", "public.png"]
    );
    // A concealed item is dropped from the set, the rest goes.
    let password = Item::text("hunter2").with(CONCEALED, b"");
    let a = s.on_host_change(2, vec![password.clone(), Item::text("note")]);
    assert_eq!(sent(&a).items, [Item::text("note")]);
    // Only concealed, or only transient: nothing.
    assert_eq!(s.on_host_change(3, vec![password]), [Action::Nothing]);
    let transient = Item::text("otp").with(TRANSIENT, b"");
    assert_eq!(s.on_host_change(4, vec![transient]), [Action::Nothing]);
    // The guest side filters too.
    let a = s.on_guest_change(
        1,
        vec![Item::text("g").with(CONCEALED, b""), Item::text("ok")],
    );
    assert_eq!(set(&a).items, [Item::text("ok")]);
    // An auto generated set is not re-sent.
    let again = Item::text("note").with(AUTO_GENERATED, b"");
    assert_eq!(s.on_host_change(5, vec![again]), [Action::Nothing]);
    // An item with no known type is nothing.
    assert_eq!(
        s.on_host_change(6, vec![Item::default().with("com.example.private", b"x")]),
        [Action::Nothing]
    );
    // Two items keep their order; the wire form round trips.
    let two = vec![Item::text("a"), Item::text("b").with(PNG, &[0, 255, 7])];
    let a = s.on_host_change(7, two.clone());
    assert_eq!(sent(&a).items, two);
    let wire = to_wire(&two);
    assert_eq!(wire[1].types["public.png"], "AP8H");
    assert_eq!(from_wire(&wire), two);
    assert_eq!(prepare(&[]).unwrap(), Vec::<Item>::new());
}

/// Up to 64 KiB inline, above as a transfer, above 16 MiB refused with
/// the badge; an oversize image goes first so the text still crosses.
#[test]
fn size_limits() {
    let mut s = Sync::new(shares());
    s.on_key(true);
    let small = Item::text(&"a".repeat(INLINE_MAX as usize));
    let a = s.on_host_change(1, vec![small]);
    assert!(sent(&a).inline(), "exactly 64 KiB is inline");
    let big = Item::text(&"b".repeat(INLINE_MAX as usize + 1));
    let a = s.on_host_change(2, vec![big]);
    assert!(!sent(&a).inline(), "64 KiB plus one is a transfer");
    assert_eq!(sent(&a).size(), INLINE_MAX + 1);
    let huge = Item::text(&"c".repeat(TRANSFER_MAX as usize + 1));
    let a = s.on_host_change(3, vec![huge.clone()]);
    assert_eq!(
        a,
        [Action::Refuse(text::clipboard_too_large(TRANSFER_MAX + 1))]
    );
    assert!(a[0] != Action::Nothing);
    // The guest side refuses too.
    let a = s.on_guest_change(1, vec![huge]);
    assert!(matches!(&a[0], Action::Refuse(b) if b.contains("max 16 MiB")));
    // An image over the limit with a text: the image goes, the text stays.
    let png = vec![0u8; TRANSFER_MAX as usize + 1];
    let a = s.on_host_change(4, vec![Item::text("caption").with(PNG, &png)]);
    assert_eq!(sent(&a).items, [Item::text("caption")]);
    // An image alone over the limit: the image badge.
    let a = s.on_host_change(5, vec![Item::default().with(PNG, &png).with(TIFF, &[1])]);
    assert_eq!(a, [Action::Refuse(text::image_too_large())]);
    // A PNG at the limit with a TIFF over it: the TIFF goes, the PNG
    // stays, at the limit.
    let a = s.on_host_change(
        6,
        vec![Item::default()
            .with(PNG, &png[..TRANSFER_MAX as usize])
            .with(TIFF, &png)],
    );
    let types: Vec<&str> = sent(&a).items[0]
        .types
        .iter()
        .map(|(t, _)| t.as_str())
        .collect();
    assert_eq!(types, [PNG]);
    assert_eq!(sent(&a).size(), TRANSFER_MAX);
    assert_eq!(Payload { items: vec![] }.size(), 0);
}

/// A file URL from the guest outside a share is dropped, inside a share
/// is translated to the host path; from the host, into the guest path.
#[test]
fn file_urls_translate_or_drop() {
    let mut s = Sync::new(shares());
    s.on_key(true);
    let inside = Item::default().with(
        FILE_URL,
        b"file:///Volumes/My%20Shared%20Files/ws-1/src/a%20b.txt",
    );
    let a = s.on_guest_change(1, vec![inside]);
    assert_eq!(
        set(&a).items,
        [Item::default().with(FILE_URL, b"file:///Users/me/src/web/src/a%20b.txt")]
    );
    let outside = Item::default().with(FILE_URL, b"file:///Users/paddock/Desktop/x.txt");
    assert_eq!(
        s.on_guest_change(2, vec![outside.clone()]),
        [Action::Nothing]
    );
    // The text of the same item still crosses.
    let a = s.on_guest_change(3, vec![outside.with(TEXT, b"x")]);
    assert_eq!(set(&a).items, [Item::text("x")]);
    // A URL that escapes a share with dot-dot, or is not a file URL, drops.
    for bad in [
        "file:///Volumes/My%20Shared%20Files/ws-1/../agent/paddock-guest",
        "https://example.com/a",
        "file://other-host/Volumes/My%20Shared%20Files/ws-1/a",
    ] {
        assert_eq!(
            s.on_guest_change(
                10 + bad.len() as u64,
                vec![Item::default().with(FILE_URL, bad.as_bytes())]
            ),
            [Action::Nothing],
            "{bad}"
        );
    }
    // From the host: a path in the drop share becomes the guest path, a
    // path elsewhere drops.
    let a = s.on_host_change(
        1,
        vec![Item::default().with(FILE_URL, b"file:///p/drop/1-1/x.png")],
    );
    assert_eq!(
        sent(&a).items,
        [Item::default().with(
            FILE_URL,
            b"file:///Volumes/My%20Shared%20Files/drop/1-1/x.png"
        )]
    );
    assert_eq!(
        s.on_host_change(
            2,
            vec![Item::default().with(FILE_URL, b"file:///Users/me/Desktop/y")]
        ),
        [Action::Nothing]
    );
    assert_eq!(
        url_path("file://localhost/a/b%2Fc"),
        Some(PathBuf::from("/a/b/c"))
    );
    assert_eq!(url_path("file:///a/"), Some(PathBuf::from("/a")));
    assert_eq!(url_path("file:///a%2"), None);
    assert_eq!(url_path("file:relative"), None);
    assert_eq!(path_url(Path::new("/a b/ü")), "file:///a%20b/%C3%BC");
    assert_eq!(
        url_path(&path_url(Path::new("/a b/ü"))),
        Some(PathBuf::from("/a b/ü"))
    );
}

/// A pretend image library: a PNG of the length that the TIFF names in
/// its first bytes as `png=N`, or the TIFF's own size.
fn fake_png(tiff: &[u8]) -> Option<Vec<u8>> {
    let head = std::str::from_utf8(&tiff[..tiff.len().min(24)]).unwrap_or("");
    if head.starts_with("not an image") {
        return None;
    }
    let n = head
        .strip_prefix("png=")
        .and_then(|r| r.split(';').next())
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(tiff.len());
    let mut png = b"\x89PNG".to_vec();
    png.resize(n, 0);
    Some(png)
}

#[test]
fn html_tiff_and_image_limit() {
    // HTML without plain text: the text of the HTML comes as a second
    // type, and prepare keeps both in the order of the types.
    let html = Item::default().with(
        "public.html",
        b"<html><head><style>p{}</style></head><body><h1>Hi &amp; bye</h1>\n<p>one<br>two</p><!-- x --><ul><li>a</li><li>b&nbsp;&#x41;&#66;</li></ul><script>var a = '<p>';</script>tail</body></html>",
    );
    let out = convert(vec![html], &mut fake_png);
    assert_eq!(
        std::str::from_utf8(out[0].get(TEXT).unwrap()).unwrap(),
        "Hi & bye\none\ntwo\na\nb AB\ntail"
    );
    let kept = prepare(&out).unwrap();
    assert_eq!(
        kept[0]
            .types
            .iter()
            .map(|(t, _)| t.as_str())
            .collect::<Vec<_>>(),
        vec![TEXT, HTML]
    );
    // HTML with its own plain text keeps it.
    let both = Item::text("mine").with(HTML, b"<b>theirs</b>");
    assert_eq!(convert(vec![both.clone()], &mut fake_png), vec![both]);
    // An HTML item whose text is empty gets no empty text type.
    let empty = Item::default().with(HTML, b"<p></p>");
    assert_eq!(convert(vec![empty.clone()], &mut fake_png), vec![empty]);

    // TIFF to PNG: a TIFF alone gets a PNG; one with a PNG stays; bytes
    // the library does not read stay a TIFF alone.
    let tiff = Item::default().with(TIFF, b"png=10;...");
    let out = convert(vec![tiff], &mut fake_png);
    assert_eq!(out[0].get(PNG).unwrap().len(), 10);
    assert!(out[0].has(TIFF));
    let with_png = Item::default().with(TIFF, b"png=10;").with(PNG, b"mine");
    assert_eq!(
        convert(vec![with_png.clone()], &mut fake_png),
        vec![with_png]
    );
    let bad = Item::default().with(TIFF, b"not an image at all");
    assert_eq!(convert(vec![bad.clone()], &mut fake_png), vec![bad]);

    // An image over the limit crosses as its PNG when that fits: the
    // TIFF goes, the PNG stays.
    let big = TRANSFER_MAX as usize + 1;
    let mut large = format!("png={};", 1000).into_bytes();
    large.resize(big, 0);
    let out = convert(vec![Item::default().with(TIFF, &large)], &mut fake_png);
    let kept = prepare(&out).unwrap();
    assert_eq!(kept.len(), 1);
    assert!(kept[0].has(PNG) && !kept[0].has(TIFF));
    assert_eq!(kept[0].size(), 1000);
    // An image at 16 MiB plus one byte in every format is refused with
    // the image badge.
    let mut huge = format!("png={big};").into_bytes();
    huge.resize(big, 0);
    let out = convert(vec![Item::default().with(TIFF, &huge)], &mut fake_png);
    assert_eq!(out[0].get(PNG).unwrap().len(), big);
    assert_eq!(prepare(&out), Err(text::image_too_large()));
    // A PNG alone at the limit plus one byte: the same badge.
    let png_only = Item::default().with(PNG, &vec![0u8; big]);
    assert_eq!(prepare(&[png_only]), Err(text::image_too_large()));
    // At the limit exactly it crosses.
    let at = Item::default().with(PNG, &vec![0u8; big - 1]);
    assert_eq!(prepare(std::slice::from_ref(&at)).unwrap(), vec![at]);
}

#[test]
fn html_text_cases() {
    assert_eq!(html_text("plain"), "plain");
    assert_eq!(html_text("a  <i>b</i>\n\n c"), "a b c");
    assert_eq!(html_text("<p>x</p><p>y</p>"), "x\ny");
    assert_eq!(html_text("x<br/>y<hr>z"), "x\ny\nz");
    assert_eq!(
        html_text("&unknown; &amp &#xZZ; &#60;"),
        "&unknown; &amp &#xZZ; <"
    );
    assert_eq!(html_text("<p>&quot;q&quot; &apos;a&apos;</p>"), "\"q\" 'a'");
    assert_eq!(html_text("<unclosed"), "");
    assert_eq!(html_text("<style>never</style><script>gone"), "");
    assert_eq!(
        html_text("<div><div>deep</div></div>\n\n\n<div>two</div>"),
        "deep\ntwo"
    );
}

/// `paddock app clipboard NAME --to-app off --from-app off`, live: no
/// direction moves while it is off, and turning paste on again while key
/// sends the host content that the guest missed.
#[test]
fn each_direction_turns_off_and_on_live() {
    let mut s = Sync::new(shares());
    assert_eq!(s.directions(), (true, true));
    assert_eq!(s.set_directions(false, true), [Action::Nothing]);
    assert_eq!(s.directions(), (false, true));
    assert_eq!(s.on_key(true), [Action::Nothing]);
    assert_eq!(
        s.on_host_change(1, vec![Item::text("secret")]),
        [Action::Nothing],
        "no paste into the app"
    );
    // Copy from the app still works.
    let a = s.on_guest_change(1, vec![Item::text("from app")]);
    assert_eq!(set(&a).items, [Item::text("from app")]);
    // Paste on again while key: the missed content goes once.
    let a = s.set_directions(true, false);
    assert_eq!(sent(&a).items, [Item::text("secret")]);
    assert_eq!(s.set_directions(true, false), [Action::Nothing]);
    assert_eq!(
        s.on_guest_change(2, vec![Item::text("again")]),
        [Action::Nothing],
        "no copy from the app"
    );
    // Not key: turning paste on sends nothing.
    s.set_directions(false, false);
    s.on_key(false);
    s.on_host_change(2, vec![Item::text("later")]);
    assert_eq!(s.set_directions(true, true), [Action::Nothing]);
}

/// A share added live: a file URL into it crosses after `set_shares`.
#[test]
fn file_urls_follow_the_live_shares() {
    let mut s = Sync::new(shares());
    s.on_key(true);
    let url = |p: &str| vec![Item::default().with(FILE_URL, path_url(Path::new(p)).as_bytes())];
    assert_eq!(
        s.on_host_change(1, url("/Users/me/new/a.txt")),
        [Action::Nothing]
    );
    let mut more = shares();
    more.push(Share {
        name: "ws-2".into(),
        host: PathBuf::from("/Users/me/new"),
        read_only: true,
    });
    s.set_shares(more);
    let a = s.on_host_change(2, url("/Users/me/new/b.txt"));
    let got = String::from_utf8(sent(&a).items[0].get(FILE_URL).unwrap().to_vec()).unwrap();
    assert!(
        got.ends_with("/Volumes/My%20Shared%20Files/ws-2/b.txt") || got.contains("ws-2/b.txt"),
        "{got}"
    );
}
