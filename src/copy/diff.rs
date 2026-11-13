//! A line diff of two versions of a file: the counts for the lists, and
//! the unified text for `paddock changes --diff`.
//!
//! paddock makes the diff itself. The copy is written by an agent, so no
//! outside program reads it for this.

/// More differing lines than this: the diff shows the whole file as
/// removed and added.
const MAX_STEPS: usize = 1000;

/// The bytes look like text: no NUL byte in the first 8000.
pub fn is_text(bytes: &[u8]) -> bool {
    !bytes.iter().take(8000).any(|b| *b == 0)
}

/// The lines, each with its line end.
pub fn lines(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split_inclusive(|b| *b == b'\n').collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// The line `old` of the old text is the line `new` of the new one.
    Same(usize, usize),
    /// The old line is gone.
    Del(usize),
    /// The new line is new.
    Add(usize),
}

/// The steps from `a` to `b`, in order.
pub fn ops(a: &[&[u8]], b: &[&[u8]]) -> Vec<Op> {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let post = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[pre..a.len() - post], &b[pre..b.len() - post]);
    let mut out: Vec<Op> = (0..pre).map(|i| Op::Same(i, i)).collect();
    let mid = myers(am, bm).unwrap_or_else(|| {
        (0..am.len())
            .map(Op::Del)
            .chain((0..bm.len()).map(Op::Add))
            .collect()
    });
    out.extend(mid.into_iter().map(|op| match op {
        Op::Same(i, j) => Op::Same(i + pre, j + pre),
        Op::Del(i) => Op::Del(i + pre),
        Op::Add(j) => Op::Add(j + pre),
    }));
    out.extend((0..post).map(|i| Op::Same(a.len() - post + i, b.len() - post + i)));
    out
}

/// The shortest edit script (Myers, 1986). None when it needs more than
/// `MAX_STEPS` edits.
fn myers(a: &[&[u8]], b: &[&[u8]]) -> Option<Vec<Op>> {
    let (n, m) = (a.len() as i64, b.len() as i64);
    if n == 0 || m == 0 {
        return Some(
            (0..a.len())
                .map(Op::Del)
                .chain((0..b.len()).map(Op::Add))
                .collect(),
        );
    }
    let max = ((n + m) as usize).min(MAX_STEPS);
    let off = max as i64 + 1;
    let mut v = vec![0i64; 2 * max + 3];
    // The state of `v` before each round, for the diagonals -d-1..=d+1.
    let mut trace: Vec<Vec<i64>> = Vec::new();
    let mut found = None;
    'rounds: for d in 0..=max as i64 {
        trace.push(v[(off - d - 1) as usize..=(off + d + 1) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let mut x =
                if k == -d || (k != d && v[(off + k - 1) as usize] < v[(off + k + 1) as usize]) {
                    v[(off + k + 1) as usize]
                } else {
                    v[(off + k - 1) as usize] + 1
                };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[(off + k) as usize] = x;
            if x >= n && y >= m {
                found = Some(d);
                break 'rounds;
            }
            k += 2;
        }
    }
    let d_end = found?;
    let mut out = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..=d_end).rev() {
        let t = &trace[d as usize];
        let get = |k: i64| t[(k + d + 1) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && get(k - 1) < get(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = get(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            out.push(Op::Same((x - 1) as usize, (y - 1) as usize));
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            if x == prev_x {
                out.push(Op::Add((y - 1) as usize));
            } else {
                out.push(Op::Del((x - 1) as usize));
            }
        }
        x = prev_x;
        y = prev_y;
    }
    out.reverse();
    Some(out)
}

/// Lines added and removed from `old` to `new`.
pub fn stat(old: &[u8], new: &[u8]) -> (usize, usize) {
    let (a, b) = (lines(old), lines(new));
    let steps = ops(&a, &b);
    (
        steps.iter().filter(|o| matches!(o, Op::Add(_))).count(),
        steps.iter().filter(|o| matches!(o, Op::Del(_))).count(),
    )
}

fn push_line(out: &mut String, sign: char, line: &[u8]) {
    out.push(sign);
    let text = String::from_utf8_lossy(line);
    let body = text.strip_suffix('\n').unwrap_or(&text);
    // A control character must not reach the terminal as it is.
    out.extend(body.chars().map(|c| match c {
        '\t' => '\t',
        c if c.is_control() => '\u{fffd}',
        c => c,
    }));
    out.push('\n');
    if !text.ends_with('\n') {
        out.push_str("\\ No newline at end of file\n");
    }
}

/// The unified diff from `old` to `new` with `context` lines around each
/// change. Empty when both are the same.
pub fn unified(path: &str, old: &[u8], new: &[u8], context: usize) -> String {
    let (a, b) = (lines(old), lines(new));
    let steps = ops(&a, &b);
    let changed: Vec<usize> = steps
        .iter()
        .enumerate()
        .filter(|(_, o)| !matches!(o, Op::Same(..)))
        .map(|(i, _)| i)
        .collect();
    if changed.is_empty() {
        return String::new();
    }
    let mut out = format!("--- your folder/{path}\n+++ the copy/{path}\n");
    let mut i = 0;
    while i < changed.len() {
        let start = changed[i].saturating_sub(context);
        let mut end = (changed[i] + context + 1).min(steps.len());
        // Changes that are near each other share one block.
        while i + 1 < changed.len() && changed[i + 1] <= end + context {
            i += 1;
            end = (changed[i] + context + 1).min(steps.len());
        }
        i += 1;
        let block = &steps[start..end];
        let old_n = block.iter().filter(|o| !matches!(o, Op::Add(_))).count();
        let new_n = block.iter().filter(|o| !matches!(o, Op::Del(_))).count();
        // The first old and new line of the block, counted from the steps
        // before it.
        let old_at = steps[..start]
            .iter()
            .filter(|o| !matches!(o, Op::Add(_)))
            .count();
        let new_at = steps[..start]
            .iter()
            .filter(|o| !matches!(o, Op::Del(_)))
            .count();
        let at = |first: usize, n: usize| if n == 0 { first } else { first + 1 };
        out.push_str(&format!(
            "@@ -{},{old_n} +{},{new_n} @@\n",
            at(old_at, old_n),
            at(new_at, new_n)
        ));
        for op in block {
            match *op {
                Op::Same(x, _) => push_line(&mut out, ' ', a[x]),
                Op::Del(x) => push_line(&mut out, '-', a[x]),
                Op::Add(y) => push_line(&mut out, '+', b[y]),
            }
        }
    }
    out
}
