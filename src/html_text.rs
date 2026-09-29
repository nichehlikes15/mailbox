//! Turns email bodies into something sane to put in a single text element.
//!
//! Raw HTML emails are often 50-200 KB of minified markup on a handful of
//! enormous lines. Feeding that straight into gpui text layout is what made
//! opening emails so heavy, so bodies go through here first.

/// Max characters shown for one email body. Anything past this is cut off.
pub const MAX_BODY_CHARS: usize = 60_000;

/// Plain-text version of an email body, ready to render.
pub fn display_body(body: &str) -> String {
    let text = if looks_like_html(body) {
        html_to_text(body)
    } else {
        clean_text(body)
    };
    truncate(text, MAX_BODY_CHARS)
}

pub fn looks_like_html(text: &str) -> bool {
    let head: String = text
        .chars()
        .take(4096)
        .collect::<String>()
        .to_ascii_lowercase();
    [
        "<html",
        "<body",
        "<div",
        "<table",
        "<br",
        "<p>",
        "<p ",
        "<span",
        "<!doctype",
    ]
    .iter()
    .any(|marker| head.contains(marker))
}

pub fn html_to_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len() / 3);
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'<' => {
                if lower[i..].starts_with("<!--") {
                    i = match lower[i + 4..].find("-->") {
                        Some(end) => i + 4 + end + 3,
                        None => bytes.len(),
                    };
                    continue;
                }

                let Some(close) = lower[i..].find('>') else {
                    break;
                };
                let tag = &lower[i + 1..i + close];
                i += close + 1;

                let closing = tag.starts_with('/');
                let name: String = tag
                    .trim_start_matches('/')
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                let self_closing = tag.ends_with('/');

                match name.as_str() {
                    // Skip the contents of these entirely.
                    "script" | "style" | "head" | "title" | "noscript" | "template"
                        if !closing && !self_closing =>
                    {
                        let end_tag = format!("</{name}");
                        i = match lower[i..].find(&end_tag) {
                            Some(start) => match lower[i + start..].find('>') {
                                Some(gt) => i + start + gt + 1,
                                None => bytes.len(),
                            },
                            None => bytes.len(),
                        };
                    }
                    "br" => out.push('\n'),
                    "li" if !closing => out.push_str("\n• "),
                    "p" | "div" | "tr" | "li" | "ul" | "ol" | "table" | "blockquote"
                    | "section" | "article" | "header" | "footer" | "h1" | "h2" | "h3" | "h4"
                    | "h5" | "h6" | "hr" | "center" => out.push('\n'),
                    "td" | "th" => out.push(' '),
                    _ => {}
                }
            }
            b'&' => {
                let rest = &html[i..];
                let end = rest.bytes().take(12).position(|b| b == b';');
                match end.and_then(|end| decode_entity(&rest[1..end]).map(|c| (c, end))) {
                    Some((decoded, end)) => {
                        out.push(decoded);
                        i += end + 1;
                    }
                    None => {
                        out.push('&');
                        i += 1;
                    }
                }
            }
            _ => {
                let ch = html[i..].chars().next().unwrap_or(' ');
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }

    clean_text(&out)
}

fn decode_entity(entity: &str) -> Option<char> {
    if let Some(num) = entity.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse::<u32>().ok()?,
        };
        return char::from_u32(code);
    }

    Some(match entity {
        "nbsp" => ' ',
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "rsquo" => '\u{2019}',
        "lsquo" => '\u{2018}',
        "rdquo" => '\u{201D}',
        "ldquo" => '\u{201C}',
        "mdash" => '\u{2014}',
        "ndash" => '\u{2013}',
        "hellip" => '\u{2026}',
        "bull" => '\u{2022}',
        "middot" => '\u{00B7}',
        "copy" => '\u{00A9}',
        "reg" => '\u{00AE}',
        "trade" => '\u{2122}',
        "euro" => '\u{20AC}',
        "pound" => '\u{00A3}',
        "zwnj" | "zwj" | "shy" => '\u{200B}',
        _ => return None,
    })
}

/// Normalises newlines, drops invisible filler characters (newsletters stuff
/// hundreds of these into preheaders), collapses runs of spaces and blank lines.
pub fn clean_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0;

    for line in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        let mut cleaned = String::with_capacity(line.len());
        let mut last_space = true;
        for ch in line.chars() {
            if is_invisible(ch) {
                continue;
            }
            if ch.is_whitespace() {
                if !last_space {
                    cleaned.push(' ');
                    last_space = true;
                }
            } else {
                cleaned.push(ch);
                last_space = false;
            }
        }
        let cleaned = cleaned.trim_end();

        if cleaned.is_empty() {
            blank_run += 1;
            if blank_run > 1 || out.is_empty() {
                continue;
            }
        } else {
            blank_run = 0;
        }

        out.push_str(cleaned);
        out.push('\n');
    }

    out.trim_end().to_string()
}

pub fn is_invisible(ch: char) -> bool {
    matches!(
        ch,
        '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{034F}' | '\u{00AD}'
    )
}

fn truncate(text: String, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((byte_index, _)) => {
            let mut text = text;
            text.truncate(byte_index);
            text.push_str("\n\n[… message truncated]");
            text
        }
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags_scripts_and_entities() {
        let html = "<html><head><title>x</title><style>p{color:red}</style></head>\
            <body><!-- hi --><p>Hello&nbsp;<b>world</b> &amp; friends</p>\
            <script>alert(1)</script><div>Line&#39;two&#x21;</div><br>end</body></html>";
        assert_eq!(
            html_to_text(html),
            "Hello world & friends\n\nLine'two!\n\nend"
        );
    }

    #[test]
    fn handles_unicode_and_broken_markup() {
        assert_eq!(
            html_to_text("<p>café ☕ &bogus; & done</p><b"),
            "café ☕ &bogus; & done"
        );
    }

    #[test]
    fn collapses_whitespace_and_invisible_chars() {
        assert_eq!(
            clean_text("a  \t b\r\n\r\n\r\n\u{200C}\u{034F}c\n"),
            "a b\n\nc"
        );
    }

    #[test]
    fn truncates_long_bodies() {
        let long = "é".repeat(MAX_BODY_CHARS + 10);
        let shown = display_body(&long);
        assert!(shown.starts_with(&"é".repeat(MAX_BODY_CHARS)));
        assert!(shown.ends_with("[… message truncated]"));
    }

    #[test]
    fn detects_html() {
        assert!(looks_like_html("<DIV>hi</DIV>"));
        assert!(!looks_like_html("just text with a < sign"));
    }
}
