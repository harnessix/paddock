// Render the documentation for a terminal without changing the source Markdown.
pub(super) fn render(text: &str) -> String {
    let mut out = String::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let trimmed = line.trim();
        if trimmed.starts_with("<p ") || trimmed == "</p>" {
            continue;
        }
        if trimmed.contains("<img ") {
            let attr = |key: &str| -> Option<&str> {
                let tail = trimmed.split_once(&format!("{key}=\""))?.1;
                Some(tail.split_once('"')?.0)
            };
            if let (Some(alt), Some(src)) = (attr("alt"), attr("src")) {
                out.push_str(&format!("{alt} ({src})\n"));
            }
            continue;
        }
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            let cells: Vec<_> = trimmed
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            if cells
                .iter()
                .all(|c| c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
            {
                continue;
            }
            out.push_str(&inline(&cells.join("  —  ")));
        } else {
            let line = if line.starts_with('#') {
                line.trim_start_matches('#').trim_start()
            } else {
                line
            };
            out.push_str(&inline(line));
        }
        out.push('\n');
    }
    out
}

fn inline(line: &str) -> String {
    let mut s = line.to_string();
    // Remove only HTML anchors, leaving their surrounding text intact.
    while let Some(start) = s.find("<a ") {
        let Some(end) = s[start..].find('>') else {
            break;
        };
        s.replace_range(start..start + end + 1, "");
    }
    s = s.replace("</a>", "");
    // Keep link labels and destinations visible.
    while let Some(start) = s.find("](") {
        let Some(open) = s[..start].rfind('[') else {
            break;
        };
        let Some(end) = s[start + 2..].find(')') else {
            break;
        };
        s.replace_range(start..start + 2, " (");
        s.remove(open);
        let _ = end;
    }
    s.replace('`', "").replace("**", "")
}
