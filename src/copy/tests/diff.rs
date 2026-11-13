//! The line diff.

use crate::copy::diff::{is_text, lines, ops, stat, unified, Op};

/// Carry out the steps on `a`: the result must be `b`.
fn replay(a: &[&[u8]], b: &[&[u8]], steps: &[Op]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let (mut x, mut y) = (0, 0);
    for s in steps {
        match *s {
            Op::Same(i, j) => {
                assert_eq!((i, j), (x, y), "steps are in order");
                assert_eq!(a[i], b[j]);
                out.push(a[i].to_vec());
                x += 1;
                y += 1;
            }
            Op::Del(i) => {
                assert_eq!(i, x);
                x += 1;
            }
            Op::Add(j) => {
                assert_eq!(j, y);
                out.push(b[j].to_vec());
                y += 1;
            }
        }
    }
    assert_eq!((x, y), (a.len(), b.len()));
    out
}

fn check(old: &str, new: &str) -> (usize, usize) {
    let (a, b) = (lines(old.as_bytes()), lines(new.as_bytes()));
    let steps = ops(&a, &b);
    let got = replay(&a, &b, &steps);
    assert_eq!(got.concat(), new.as_bytes(), "{old:?} -> {new:?}");
    stat(old.as_bytes(), new.as_bytes())
}

#[test]
fn the_counts_of_added_and_removed_lines() {
    assert_eq!(check("a\nb\nc\n", "a\nb\nc\n"), (0, 0));
    assert_eq!(check("a\nb\nc\n", "a\nB\nc\n"), (1, 1));
    assert_eq!(check("a\nb\nc\n", "a\nc\n"), (0, 1));
    assert_eq!(check("a\nc\n", "a\nb\nc\n"), (1, 0));
    assert_eq!(check("", "a\nb\n"), (2, 0));
    assert_eq!(check("a\nb\n", ""), (0, 2));
    assert_eq!(check("", ""), (0, 0));
    assert_eq!(check("a\nb\nc\nd\n", "x\na\nb\nc\nd\ny\n"), (2, 0));
    assert_eq!(check("x\na\nb\ny\n", "a\nb\n"), (0, 2));
    // The last line without a line end is another line.
    assert_eq!(check("a\nb", "a\nb\n"), (1, 1));
}

#[test]
fn the_steps_give_the_new_text_for_many_pairs() {
    // A fixed sequence of numbers: every run checks the same pairs.
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    for _ in 0..300 {
        let len = next(30);
        let old: Vec<String> = (0..len).map(|_| format!("line {}\n", next(6))).collect();
        let mut new = old.clone();
        for _ in 0..next(8) {
            let at = next(new.len() as u64 + 1) as usize;
            match next(3) {
                0 if at < new.len() => {
                    new.remove(at);
                }
                1 => new.insert(at, format!("new {}\n", next(6))),
                _ if at < new.len() => new[at] = format!("line {}\n", next(6)),
                _ => {}
            }
        }
        let (added, removed) = check(&old.concat(), &new.concat());
        assert_eq!(old.len() + added - removed, new.len());
    }
}

#[test]
fn a_very_different_pair_still_gives_a_correct_diff() {
    let old: String = (0..3000).map(|i| format!("old {i}\n")).collect();
    let new: String = (0..3000).map(|i| format!("new {i}\n")).collect();
    assert_eq!(check(&old, &new), (3000, 3000));
}

#[test]
fn the_unified_text_has_blocks_with_line_numbers() {
    let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n16\n17\n18\n19\n20\n";
    let new = old
        .replace("\n3\n", "\nthree\n")
        .replace("\n18\n", "\neighteen\nand more\n");
    let d = unified("src/a.txt", old.as_bytes(), new.as_bytes(), 3);
    let want = "\
--- your folder/src/a.txt
+++ the copy/src/a.txt
@@ -1,6 +1,6 @@
 1
 2
-3
+three
 4
 5
 6
@@ -15,6 +15,7 @@
 15
 16
 17
-18
+eighteen
+and more
 19
 20
";
    assert_eq!(d, want);
    assert_eq!(unified("a", b"same\n", b"same\n", 3), "");
    // A new file and a deleted file.
    assert_eq!(
        unified("n", b"", b"a\nb\n", 3),
        "--- your folder/n\n+++ the copy/n\n@@ -0,0 +1,2 @@\n+a\n+b\n"
    );
    assert_eq!(
        unified("n", b"a\n", b"", 3),
        "--- your folder/n\n+++ the copy/n\n@@ -1,1 +0,0 @@\n-a\n"
    );
    // No line end at the end, and a control character.
    let d = unified("n", b"a\n", b"a\nb\x1b[31m", 3);
    assert!(
        d.contains("+b\u{fffd}[31m\n\\ No newline at end of file\n"),
        "{d}"
    );
    assert!(!d.contains('\x1b'));
}

#[test]
fn text_and_binary() {
    assert!(is_text(b"plain\n"));
    assert!(is_text("ünïcödé\n".as_bytes()));
    assert!(!is_text(b"PNG\0\x01\x02"));
    assert!(is_text(b""));
}
