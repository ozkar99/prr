//! Markdown helpers. (Rendering comes with the review checkpoint.)
use regex::Regex;
use std::sync::OnceLock;

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

/// First meaningful line of a Markdown/HTML comment as plain text, for one-line
/// previews (port of the web version's review/snippet.ts).
pub fn snippet(markdown: &str) -> String {
    static HTML_COMMENT: OnceLock<Regex> = OnceLock::new();
    static FENCE: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    static IMAGE: OnceLock<Regex> = OnceLock::new();
    static LINK: OnceLock<Regex> = OnceLock::new();
    static QUOTE: OnceLock<Regex> = OnceLock::new();
    static HEADING: OnceLock<Regex> = OnceLock::new();
    static LIST: OnceLock<Regex> = OnceLock::new();
    static RULE: OnceLock<Regex> = OnceLock::new();
    static EMPHASIS: OnceLock<Vec<Regex>> = OnceLock::new();

    let text = re(&HTML_COMMENT, r"(?s)<!--.*?-->").replace_all(markdown, ""); // bots hide metadata here
    let text = re(&FENCE, r"(?s)```.*?```").replace_all(&text, " ");
    let text = re(&TAG, r"<[^>]+>").replace_all(&text, " ");
    let text = re(&IMAGE, r"!\[[^\]]*\]\([^)]*\)").replace_all(&text, "");
    let text = re(&LINK, r"\[([^\]]*)\]\([^)]*\)").replace_all(&text, "$1");
    let emphasis = EMPHASIS.get_or_init(|| {
        [r"\*\*(.+?)\*\*", r"__(.+?)__", r"\*(.+?)\*", r"_(.+?)_", r"~~(.+?)~~", r"`(.+?)`"]
            .iter()
            .map(|p| Regex::new(p).unwrap())
            .collect()
    });
    for raw in text.lines() {
        let line = re(&QUOTE, r"^\s*(?:>\s*)+").replace(raw, "");
        let line = re(&HEADING, r"^\s*#{1,6}\s+").replace(&line, "");
        let line = re(&LIST, r"^\s*(?:[-*+]|\d+\.)\s+(?:\[[ xX]\]\s+)?").replace(&line, "");
        let mut line = line.into_owned();
        for r in emphasis {
            line = r.replace_all(&line, "$1").into_owned();
        }
        let line = line.trim();
        if !line.is_empty() && !re(&RULE, r"^[-*_=\s|:]+$").is_match(line) {
            return line.to_string();
        }
    }
    String::new()
}
