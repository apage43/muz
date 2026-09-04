use anyhow::Result;
/// Conservative formatting: retain layout choices and comments, normalize indentation.
pub fn format(source: &str) -> Result<String> {
    super::parser::parse(source)?;
    let mut out = String::new();
    let mut depth = 0usize;
    let mut quote = None;
    let mut block = false;
    let mut escape = false;
    for line in source.lines() {
        let inside = quote.is_some() || block;
        let trimmed = if inside { line } else { line.trim() };
        if !inside && !trimmed.is_empty() {
            let closing = trimmed
                .chars()
                .take_while(|c| matches!(c, '}' | ']' | ')'))
                .count();
            out.push_str(&"    ".repeat(depth.saturating_sub(closing)));
        }
        out.push_str(trimmed);
        out.push('\n');
        let bytes = trimmed.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            if let Some(q) = quote {
                if escape {
                    escape = false;
                } else if c == b'\\' {
                    escape = true;
                } else if c == q {
                    quote = None;
                }
                i += 1;
                continue;
            }
            if block {
                if c == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    block = false;
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            if c == b'/' && bytes.get(i + 1) == Some(&b'/') {
                break;
            }
            if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
                block = true;
                i += 2;
                continue;
            }
            match c {
                b'"' | b'\'' => quote = Some(c),
                b'{' | b'[' | b'(' => depth += 1,
                b'}' | b']' | b')' => depth = depth.saturating_sub(1),
                _ => {}
            }
            i += 1;
        }
    }
    super::parser::parse(&out)?;
    Ok(out)
}
