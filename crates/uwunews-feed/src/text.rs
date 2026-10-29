//! HTML to the plain text a list row shows.
//!
//! Not a sanitiser: the reader renders `content_html` in a web view with
//! scripts off. This is only for the one or two lines under a title, so it
//! drops tags, decodes the common entities and folds whitespace.

/// Plain text of an HTML fragment, at most `max` characters, cut at a word.
pub fn plain(html: &str, max: usize) -> String {
    let mut out = String::with_capacity(html.len().min(max * 2));
    let mut chars = html.chars().peekable();
    let mut skip_until: Option<&str> = None;
    let mut space = false;
    while let Some(c) = chars.next() {
        if c == '<' {
            let mut tag = String::new();
            for t in chars.by_ref() {
                if t == '>' {
                    break;
                }
                tag.push(t);
            }
            let name: String = tag
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase();
            if let Some(end) = skip_until {
                if tag.starts_with('/') && name == end {
                    skip_until = None;
                }
                continue;
            }
            match name.as_str() {
                "script" => skip_until = Some("script"),
                "style" => skip_until = Some("style"),
                _ => {}
            }
            space = true;
            continue;
        }
        if skip_until.is_some() {
            continue;
        }
        let c = if c == '&' {
            let mut entity = String::new();
            while let Some(&e) = chars.peek() {
                if e == ';' || entity.len() > 8 {
                    break;
                }
                entity.push(e);
                chars.next();
            }
            if chars.peek() == Some(&';') {
                chars.next();
                match decode_entity(&entity) {
                    Some(d) => d,
                    None => continue,
                }
            } else {
                if space && !out.is_empty() {
                    out.push(' ');
                }
                space = false;
                out.push('&');
                out.push_str(&entity);
                continue;
            }
        } else {
            c
        };
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(c);
        if out.chars().count() > max + 40 {
            break;
        }
    }
    cut(&out, max)
}

fn decode_entity(entity: &str) -> Option<char> {
    Some(match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" | "#39" => '\'',
        "nbsp" => ' ',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "laquo" => '«',
        "raquo" => '»',
        "bdquo" => '„',
        "ldquo" => '“',
        "rdquo" => '”',
        "lsquo" => '‘',
        "rsquo" => '’',
        "auml" => 'ä',
        "ouml" => 'ö',
        "uuml" => 'ü',
        "Auml" => 'Ä',
        "Ouml" => 'Ö',
        "Uuml" => 'Ü',
        "szlig" => 'ß',
        "euro" => '€',
        e if e.starts_with("#x") || e.starts_with("#X") => {
            char::from_u32(u32::from_str_radix(&e[2..], 16).ok()?)?
        }
        e if e.starts_with('#') => char::from_u32(e[1..].parse().ok()?)?,
        _ => return None,
    })
}

/// At most `max` characters, at the last space before the limit, with an
/// ellipsis when something was cut.
pub fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    let head = match head.rfind(' ') {
        Some(at) if at > max / 2 => &head[..at],
        _ => head.as_str(),
    };
    format!("{}…", head.trim_end_matches([',', '.', ';', ':', ' ']))
}

/// The first `<img src="…">` of an HTML fragment, if it is an http(s) URL.
pub fn first_image(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find("<img") {
        let start = from + at;
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        let tag = &html[start..end];
        if let Some(src) = attribute(tag, "src") {
            if src.starts_with("https://") || src.starts_with("http://") {
                return Some(src);
            }
        }
        from = end;
    }
    None
}

/// An attribute's value in a tag's text, quoted with `"` or `'`.
pub fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find(name) {
        let start = from + at;
        from = start + name.len();
        let before_ok = start == 0 || lower.as_bytes()[start - 1].is_ascii_whitespace();
        let rest = lower[from..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let value_at = tag.len() - rest.len() + 1;
        let value = tag[value_at..].trim_start();
        let quote = value.chars().next()?;
        let value = if quote == '"' || quote == '\'' {
            let inner = &value[1..];
            &inner[..inner.find(quote)?]
        } else {
            value
                .split(|c: char| c.is_whitespace() || c == '>')
                .next()?
        };
        return Some(value.replace("&amp;", "&"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags_and_scripts() {
        let text = plain(
            "<p>Hallo <b>Welt</b></p><script>alert(1)</script><p>&amp; mehr&nbsp;hier</p>",
            100,
        );
        assert_eq!(text, "Hallo Welt & mehr hier");
    }

    #[test]
    fn cuts_at_a_word() {
        assert_eq!(cut("eins zwei drei vier", 12), "eins zwei…");
        assert_eq!(cut("kurz", 12), "kurz");
    }

    #[test]
    fn numeric_entities_and_garbage() {
        assert_eq!(plain("&#228;&#x00FC; &bogus; &", 50), "äü &");
    }

    #[test]
    fn finds_the_first_web_image() {
        let html =
            r#"<img src="data:x"><IMG alt='a' SRC='https://img.example.com/a.png?x=1&amp;y=2'>"#;
        assert_eq!(
            first_image(html).as_deref(),
            Some("https://img.example.com/a.png?x=1&y=2")
        );
    }
}
