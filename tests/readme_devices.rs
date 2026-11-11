//! The "Devices: /dev/paddock" section of the README: every TOML block
//! parses with the parsers of paddock, the worked example in it is the
//! one that `paddock device add --example mac` writes, and no sentence
//! of the section is longer than 25 words.

use paddock::devices::DevicesCfg;

fn section() -> String {
    let readme = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/help/guide-devices.md"
    ))
    .unwrap();
    let rest = readme.as_str();
    assert!(rest.starts_with("## Devices: /dev/paddock\n"));
    // Up to "Wait without sleep", which is about blocking reads, not the
    // devices themselves.
    let end = rest
        .find("\n### Wait without sleep\n")
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

/// The fenced blocks of the section with their language.
fn blocks(text: &str) -> Vec<(String, String)> {
    let mut v = Vec::new();
    let mut lines = text.lines();
    while let Some(l) = lines.next() {
        if let Some(lang) = l.strip_prefix("```") {
            let mut body = String::new();
            for b in lines.by_ref() {
                if b.starts_with("```") {
                    break;
                }
                body.push_str(b);
                body.push('\n');
            }
            v.push((lang.trim().to_string(), body));
        }
    }
    v
}

#[test]
fn every_toml_block_of_the_devices_section_parses() {
    let text = section();
    let toml_blocks: Vec<String> = blocks(&text)
        .into_iter()
        .filter(|(lang, _)| lang == "toml")
        .map(|(_, b)| b)
        .collect();
    assert!(toml_blocks.len() >= 8, "{} toml blocks", toml_blocks.len());
    for b in &toml_blocks {
        let cfg = DevicesCfg::parse(b).unwrap_or_else(|e| panic!("{e:#}\n{b}"));
        if b.contains("[devices.") {
            assert!(!cfg.user.is_empty(), "{b}");
        }
        if b.contains("[credentials.") {
            paddock::credentials::parse(b).unwrap_or_else(|e| panic!("{e:#}\n{b}"));
        }
        if b.contains("[pastures.") {
            let v: toml::Value = toml::from_str(b).unwrap();
            let p = &v["pastures"]["workers"];
            assert_eq!(p["devices"], toml::Value::Array(vec!["mac".into()]), "{b}");
        }
    }
}

#[test]
fn the_worked_example_of_the_readme_is_the_one_that_add_writes() {
    use paddock::devices::examples::{MacExample, BUILD_SH};
    let text = section();
    let bs = blocks(&text);
    let readme_tables = bs
        .iter()
        .find(|(l, b)| l == "toml" && b.contains("[credentials.mac-ssh]"))
        .map(|(_, b)| b.clone())
        .expect("the example tables in the README");
    let readme_script = bs
        .iter()
        .find(|(l, b)| l == "sh" && b.contains("rsync -a --delete"))
        .map(|(_, b)| b.clone())
        .expect("the script in the README");
    // The script: the README shows the lines without the comment block.
    for line in readme_script.lines() {
        assert!(BUILD_SH.contains(line), "not in build.sh: {line}");
    }
    // The tables: the same keys and values as `add` writes, with `~` in
    // the README where `add` writes the home path, and without the
    // pasture table that `add` never writes.
    let ex = MacExample::new("mac.local", "me", std::path::Path::new("/home/demo"));
    let written = ex.tables();
    let strip = |t: &str| -> Vec<String> {
        t.lines()
            .map(|l| l.split('#').next().unwrap().trim().to_string())
            .filter(|l| {
                !l.is_empty()
                    && !l.starts_with("[pastures")
                    && l != "members = [\"codex\", \"claude\"]"
                    && l != "devices = [\"mac\"]"
            })
            .map(|l| l.replace("/home/demo/", "~/"))
            .collect()
    };
    assert_eq!(strip(&readme_tables), strip(&written));
}

#[test]
fn no_sentence_of_the_devices_section_is_over_25_words() {
    let text = section();
    // Prose only: no fenced blocks, no code spans, no table rules.
    let mut prose = String::new();
    let mut in_block = false;
    for l in text.lines() {
        if l.starts_with("```") {
            in_block = !in_block;
            continue;
        }
        if in_block || l.starts_with("| :---") {
            continue;
        }
        // Table cells are sentences of their own.
        let l = if l.starts_with('|') {
            l.replace('|', ". ")
        } else {
            l.to_string()
        };
        prose.push_str(&l);
        prose.push('\n');
    }
    let mut no_code = String::new();
    let mut in_span = false;
    for c in prose.chars() {
        if c == '`' {
            in_span = !in_span;
            no_code.push(' ');
            continue;
        }
        no_code.push(if in_span { ' ' } else { c });
    }
    let mut long = Vec::new();
    for sentence in no_code.split(['.', '?', '!', '\n']) {
        let words = sentence
            .split_whitespace()
            .filter(|w| w.chars().any(char::is_alphanumeric))
            .count();
        if words > 25 {
            long.push(format!("{words} words: {}", sentence.trim()));
        }
    }
    assert!(
        long.is_empty(),
        "sentences over 25 words:\n{}",
        long.join("\n")
    );
}
