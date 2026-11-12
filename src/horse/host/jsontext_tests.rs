use super::*;
use serde_json::json;

fn check(text: &str, i: &Insert) -> String {
    let new = i.apply(text);
    // The new text is valid JSON, and without the insertion it is the old.
    serde_json::from_str::<Value>(&new).unwrap_or_else(|e| panic!("{e}: {new}"));
    let mut back = new.clone();
    back.replace_range(i.at..i.at + i.text.len(), "");
    assert_eq!(back, text);
    new
}

#[test]
fn spans_of_members_and_items() {
    let t = r#" { "a" : [1, "x\"]", {"b": null}], "c": {} } "#;
    let root = parse(t).unwrap();
    let a = root.get("a").unwrap();
    assert_eq!(&t[a.start..a.end], r#"[1, "x\"]", {"b": null}]"#);
    let items = a.items().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(&t[items[1].start..items[1].end], r#""x\"]""#);
    assert_eq!(&t[items[2].start..items[2].end], r#"{"b": null}"#);
    assert!(root.get("c").unwrap().members().unwrap().is_empty());
    assert!(parse("{").is_err());
    assert!(parse("{} x").is_err());
}

#[test]
fn an_item_follows_the_layout_of_its_list() {
    // Four spaces, one item per line.
    let t = "{\n    \"list\": [\n        1,\n        2\n    ]\n}\n";
    let root = parse(t).unwrap();
    let i = append_item(t, root.get("list").unwrap(), &json!({"k": [3]})).unwrap();
    let new = check(t, &i);
    assert_eq!(
        new,
        "{\n    \"list\": [\n        1,\n        2,\n        {\n            \"k\": [\n                3\n            ]\n        }\n    ]\n}\n"
    );
    // One line: the compact form.
    let t = r#"{"list":[1, 2]}"#;
    let root = parse(t).unwrap();
    let new = check(
        t,
        &append_item(t, root.get("list").unwrap(), &json!({"k": 3})).unwrap(),
    );
    assert_eq!(new, r#"{"list":[1, 2, {"k":3}]}"#);
    // Tabs.
    let t = "{\n\t\"list\": [\n\t\t1\n\t]\n}";
    let root = parse(t).unwrap();
    let new = check(
        t,
        &append_item(t, root.get("list").unwrap(), &json!([2])).unwrap(),
    );
    assert_eq!(
        new,
        "{\n\t\"list\": [\n\t\t1,\n\t\t[\n\t\t\t2\n\t\t]\n\t]\n}"
    );
}

#[test]
fn a_member_goes_at_the_end_of_its_object() {
    let t = "{\n  \"z\": 1,\n  \"a\": 2\n}";
    let root = parse(t).unwrap();
    let new = check(t, &append_member(t, &root, "m", &json!(true)).unwrap());
    assert_eq!(new, "{\n  \"z\": 1,\n  \"a\": 2,\n  \"m\": true\n}");
    // An empty object of a text with lines gets a line of its own.
    let t = "{\n  \"hooks\": {}\n}";
    let root = parse(t).unwrap();
    let new = check(
        t,
        &append_member(t, root.get("hooks").unwrap(), "Stop", &json!([])).unwrap(),
    );
    assert_eq!(new, "{\n  \"hooks\": {\n    \"Stop\": []\n  }\n}");
    // A key that is there already is refused.
    assert!(append_member(t, &root, "hooks", &json!(1)).is_err());
}

#[test]
fn a_cut_takes_the_element_and_its_comma() {
    let t = "{\n  \"l\": [\n    1,\n    2,\n    3\n  ]\n}";
    let root = parse(t).unwrap();
    let l = root.get("l").unwrap();
    assert_eq!(
        cut_item(t, l, 2).unwrap(),
        "{\n  \"l\": [\n    1,\n    2\n  ]\n}"
    );
    assert_eq!(
        cut_item(t, l, 0).unwrap(),
        "{\n  \"l\": [\n    2,\n    3\n  ]\n}"
    );
    let one = r#"{"l": [ 1 ], "m": 2}"#;
    let root = parse(one).unwrap();
    assert_eq!(
        cut_item(one, root.get("l").unwrap(), 0).unwrap(),
        r#"{"l": [], "m": 2}"#
    );
    assert_eq!(cut_member(one, &root, "m").unwrap(), r#"{"l": [ 1 ]}"#);
    assert_eq!(cut_member(one, &root, "l").unwrap(), r#"{"m": 2}"#);
}
