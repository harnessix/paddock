use super::*;

#[test]
fn the_first_instance_keeps_the_default_folder() {
    assert_eq!(folder(1), "default");
    assert_eq!(folder(0), "default");
    assert_eq!(folder(2), "instance-2");
    assert_eq!(folder(99), "instance-99");
}

#[test]
fn a_folder_name_gives_its_number_and_nothing_else_does() {
    assert_eq!(number("default"), Some(1));
    assert_eq!(number("instance-2"), Some(2));
    assert_eq!(number("instance-99"), Some(99));
    for bad in [
        "instance-1",
        "instance-0",
        "instance-100",
        "instance-02",
        "instance-",
        "instance-x",
        "work",
        "",
    ] {
        assert_eq!(number(bad), None, "{bad}");
    }
    for n in 1..=MAX {
        assert_eq!(number(&folder(n)), Some(n));
    }
}

#[test]
fn keys_and_labels_name_the_instance_after_the_first() {
    assert_eq!(key("chatgpt", 1), "chatgpt");
    assert_eq!(key("chatgpt", 3), "chatgpt--3");
    assert_eq!(label("ChatGPT", 1), "ChatGPT");
    assert_eq!(label("ChatGPT", 2), "ChatGPT 2");
    assert_eq!(arg("chatgpt", 1), "chatgpt");
    assert_eq!(arg("chatgpt", 2), "chatgpt --instance 2");
}

#[test]
fn the_profile_folder_names_its_instance() {
    let p = std::path::Path::new("/m/profiles.noindex/chatgpt");
    assert_eq!(of_profile(&p.join("default")), 1);
    assert_eq!(of_profile(&p.join("instance-4")), 4);
    assert_eq!(of_profile(&p.join("work")), 1);
}

#[test]
fn parse_takes_one_to_ninety_nine() {
    assert_eq!(parse("1"), Ok(1));
    assert_eq!(parse(" 7 "), Ok(7));
    assert_eq!(parse("99"), Ok(99));
    for bad in ["0", "100", "-1", "two", ""] {
        let e = parse(bad).unwrap_err();
        assert!(e.contains("--instance"), "{e}");
    }
}

#[test]
fn new_picks_the_lowest_number_without_a_folder() {
    assert_eq!(next_free(&[]), Some(1));
    assert_eq!(next_free(&[1]), Some(2));
    assert_eq!(next_free(&[1, 2, 4]), Some(3));
    let all: Vec<u32> = (1..=MAX).collect();
    assert_eq!(next_free(&all), None);
}

#[test]
fn choose_reads_the_two_flags() {
    assert_eq!(choose(None, false, &[1, 2]), Ok(1));
    assert_eq!(choose(Some(5), false, &[1]), Ok(5));
    assert_eq!(choose(None, true, &[1, 2]), Ok(3));
    assert!(choose(Some(2), true, &[]).unwrap_err().contains("--new"));
    let all: Vec<u32> = (1..=MAX).collect();
    assert!(choose(None, true, &all).unwrap_err().contains("99"));
}

#[test]
fn existing_lists_the_instance_folders_of_one_app() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::at(dir.path().join("macvm"));
    for (app, home) in [
        ("chatgpt", "default"),
        ("chatgpt", "instance-3"),
        ("chatgpt", "work"),
        ("other", "instance-2"),
    ] {
        std::fs::create_dir_all(layout.profile(app, home).dir).unwrap();
    }
    assert_eq!(existing(&layout, "chatgpt"), vec![1, 3]);
    assert_eq!(existing(&layout, "other"), vec![2]);
    assert!(existing(&layout, "none").is_empty());
    // The profile of instance N is the folder of `folder(N)`.
    assert_eq!(
        layout.profile("chatgpt", &folder(3)).dir,
        dir.path().join("macvm/profiles.noindex/chatgpt/instance-3")
    );
}
