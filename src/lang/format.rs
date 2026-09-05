use anyhow::{Result, ensure};

const WIDTH: usize = 100;

/// Format source without changing tokens, string contents, or comment text.
pub fn format(source: &str) -> Result<String> {
    super::parser::parse(source)?;
    let tokens = super::parser::lex(source)?;
    let mut pieces = Vec::new();
    let mut end = 0;
    for token in tokens.iter().filter(|t| t.at < source.len()) {
        trivia(&source[end..token.at], &mut pieces);
        pieces.push(Piece {
            text: source[token.at..token.end].into(),
            kind: if token.string {
                Kind::String
            } else {
                Kind::Code
            },
            lines: 0,
        });
        end = token.end;
    }
    trivia(&source[end..], &mut pieces);
    let mut pos = 0;
    let nodes = tree(&pieces, &mut pos);
    let doc = sequence(&nodes, true, false, false);
    let mut output = String::new();
    render(&doc, 0, false, &mut output, WIDTH);
    let output = format!("{}\n", output.trim_end());
    super::parser::parse(&output)?;
    // A formatter must never silently alter a musical value or operator.
    let after = super::parser::lex(&output)?;
    ensure!(
        tokens
            .iter()
            .map(|t| (&t.text, t.string))
            .eq(after.iter().map(|t| (&t.text, t.string))),
        "formatting changed source tokens"
    );
    Ok(output)
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Code,
    String,
    Comment,
    Break,
}
#[derive(Clone)]
struct Piece {
    text: String,
    kind: Kind,
    lines: usize,
}

// The lexer skips comments; recover them verbatim from the gaps between tokens.
fn trivia(mut s: &str, out: &mut Vec<Piece>) {
    while !s.is_empty() {
        if s.starts_with("//") || s.starts_with("/*") {
            let n = if s.starts_with("//") {
                s.find('\n').unwrap_or(s.len())
            } else {
                s.find("*/").unwrap() + 2
            };
            out.push(Piece {
                text: s[..n].into(),
                kind: Kind::Comment,
                lines: 0,
            });
            s = &s[n..];
        } else {
            let n = s.find(|c: char| !c.is_whitespace()).unwrap_or(s.len());
            let lines = s[..n].bytes().filter(|c| *c == b'\n').count();
            if lines > 0 {
                out.push(Piece {
                    text: String::new(),
                    kind: Kind::Break,
                    lines,
                });
            }
            s = &s[n..];
        }
    }
}

#[derive(Clone)]
enum Node {
    Atom(Piece),
    Group(String, Vec<Node>, String),
}
impl Node {
    fn text(&self) -> &str {
        match self {
            Self::Atom(p) => &p.text,
            Self::Group(open, _, _) => open,
        }
    }
    fn is_break(&self) -> bool {
        matches!(self, Self::Atom(p) if p.kind == Kind::Break)
    }
    fn comment(&self) -> bool {
        matches!(self, Self::Atom(p) if p.kind == Kind::Comment)
    }
}
fn tree(pieces: &[Piece], pos: &mut usize) -> Vec<Node> {
    let mut nodes = Vec::new();
    while *pos < pieces.len() {
        let p = &pieces[*pos];
        if p.kind == Kind::Code && matches!(p.text.as_str(), ")" | "]" | "}") {
            break;
        }
        *pos += 1;
        if p.kind == Kind::Code && matches!(p.text.as_str(), "(" | "[" | "{") {
            let inner = tree(pieces, pos);
            let close = pieces[*pos].text.clone();
            *pos += 1;
            nodes.push(Node::Group(p.text.clone(), inner, close));
        } else {
            nodes.push(Node::Atom(p.clone()));
        }
    }
    nodes
}

#[derive(Clone)]
enum Doc {
    Text(String),
    Line(&'static str),
    Hard,
    Concat(Vec<Doc>),
    Nest(Box<Doc>),
    Group(Box<Doc>),
    Chain(Box<Doc>, Box<Doc>),
    Call(Vec<(Doc, bool)>, bool),
}
fn text(s: impl Into<String>) -> Doc {
    Doc::Text(s.into())
}
fn cat(ds: Vec<Doc>) -> Doc {
    Doc::Concat(ds)
}
fn nest(d: Doc) -> Doc {
    Doc::Nest(Box::new(d))
}
fn group(d: Doc) -> Doc {
    Doc::Group(Box::new(d))
}
fn statement_doc(mut docs: Vec<Doc>) -> Doc {
    // A multiline receiver does not by itself require a multiline suffix.
    if let Some(at) = docs
        .iter()
        .position(|d| matches!(d, Doc::Nest(inner) if matches!(inner.as_ref(), Doc::Line(""))))
    {
        let suffix = docs.split_off(at);
        group(Doc::Chain(Box::new(cat(docs)), Box::new(cat(suffix))))
    } else {
        group(cat(docs))
    }
}
fn flat_width(d: &Doc) -> Option<usize> {
    match d {
        Doc::Text(s) => {
            if s.contains('\n') {
                None
            } else {
                Some(s.chars().count())
            }
        }
        Doc::Line(s) => Some(s.len()),
        Doc::Hard => None,
        Doc::Concat(ds) => ds
            .iter()
            .try_fold(0usize, |n, d| n.checked_add(flat_width(d)?)),
        Doc::Nest(d) | Doc::Group(d) => flat_width(d),
        Doc::Chain(receiver, suffix) => flat_width(receiver)?.checked_add(flat_width(suffix)?),
        Doc::Call(args, trailing) => args.iter().try_fold(
            2 + args.len().saturating_sub(1) * 2 + usize::from(*trailing),
            |n, (arg, _)| n.checked_add(flat_width(arg)?),
        ),
    }
}
fn render(d: &Doc, indent: usize, flat: bool, out: &mut String, width: usize) {
    match d {
        Doc::Text(s) => {
            if out.ends_with('\n') {
                out.push_str(&" ".repeat(indent));
            }
            out.push_str(s);
        }
        Doc::Line(s) if flat => out.push_str(s),
        Doc::Line(_) | Doc::Hard => {
            while out.ends_with(' ') {
                out.pop();
            }
            out.push('\n');
        }
        Doc::Concat(ds) => {
            for (i, d) in ds.iter().enumerate() {
                // Reserve adjacent closing delimiters when fitting an inner group.
                let reserve: usize = ds[i + 1..]
                    .iter()
                    .map_while(|next| {
                        if let Doc::Text(s) = next {
                            Some(s.chars().count())
                        } else {
                            None
                        }
                    })
                    .sum();
                render(d, indent, flat, out, width.saturating_sub(reserve));
            }
        }
        Doc::Nest(d) => render(d, indent + 4, flat, out, width),
        Doc::Call(args, trailing) => {
            render(&text("("), indent, flat, out, width);
            let mut continuation = false;
            let mut previous_multiline = false;
            for (i, (arg, collection)) in args.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let column = out.rsplit('\n').next().unwrap().chars().count();
                // A collection can expand in place, with its brace attached to the call.
                let argument_width = if *collection {
                    Some(1)
                } else {
                    flat_width(arg)
                };
                let fits =
                    argument_width.is_some_and(|n| column + usize::from(i > 0) + n + 1 <= width);
                if !flat && (!fits || previous_multiline) {
                    render(&Doc::Hard, indent, false, out, width);
                    continuation = true;
                } else if i > 0 {
                    out.push(' ');
                }
                let start = out.len();
                render(
                    arg,
                    indent + if continuation { 4 } else { 0 },
                    flat,
                    out,
                    width.saturating_sub(1),
                );
                previous_multiline = out[start..].contains('\n');
            }
            if *trailing {
                out.push(',');
            }
            out.push(')');
        }
        Doc::Chain(receiver, suffix) => {
            let start = out.len();
            render(receiver, indent, flat, out, width);
            let multiline = out[start..].contains('\n');
            let column = out.rsplit('\n').next().unwrap().chars().count();
            let attach = multiline && flat_width(suffix).is_some_and(|n| column + n <= width);
            render(suffix, indent, flat || attach, out, width);
        }
        Doc::Group(d) => {
            let column = if out.ends_with('\n') || out.is_empty() {
                indent
            } else {
                out.rsplit('\n').next().unwrap().chars().count()
            };
            render(
                d,
                indent,
                flat || flat_width(d).is_some_and(|n| column + n <= width),
                out,
                width,
            );
        }
    }
}

fn significant(nodes: &[Node]) -> Vec<&Node> {
    nodes.iter().filter(|n| !n.is_break()).collect()
}
fn operator(s: &str) -> bool {
    matches!(
        s,
        "=" | "+"
            | "-"
            | "*"
            | "/"
            | "%"
            | "=="
            | "!="
            | "<"
            | ">"
            | "<="
            | ">="
            | "&&"
            | "||"
            | "=>"
            | "|>"
    )
}
fn unary(nodes: &[&Node], i: usize) -> bool {
    matches!(nodes[i].text(), "-" | "!")
        && (i == 0
            || operator(nodes[i - 1].text())
            || matches!(nodes[i - 1].text(), "," | ":" | ";"))
}

fn sequence(nodes: &[Node], root: bool, align: bool, drum_call: bool) -> Doc {
    let sig = significant(nodes);
    // A single member call is one expression head; expand its arguments before
    // separating the receiver from the method name (voices.express({ ... })).
    let member_call = sig.len() == 4
        && matches!(sig[0], Node::Atom(p) if p.kind == Kind::Code)
        && sig[1].text() == "."
        && matches!(sig[3], Node::Group(open, _, _) if open == "(");
    let key_width = if align {
        sig.windows(2)
            .filter(|w| matches!(w[1].text(), ":" | "="))
            .map(|w| w[0].text().chars().count())
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    let mut docs = Vec::new();
    let mut statement = Vec::new();
    let mut i: usize = 0;
    let mut pending_lines = 0;
    for node in nodes {
        if let Node::Atom(p) = node {
            if p.kind == Kind::Break {
                pending_lines = pending_lines.max(p.lines);
                continue;
            }
        }
        let prev = i.checked_sub(1).map(|j| sig[j]);
        let s = node.text();
        let boundary = root
            && pending_lines > 0
            && prev.is_some_and(|p| {
                !operator(p.text())
                    && !matches!(p.text(), "," | ".")
                    && !operator(s)
                    && s != "."
                    && s != ";"
                    && s != "else"
            });
        if boundary || prev.is_some_and(|p| p.comment() && p.text().starts_with("//")) {
            if !statement.is_empty() {
                docs.push(statement_doc(std::mem::take(&mut statement)));
            }
            if !matches!(docs.last(), Some(Doc::Hard)) {
                docs.push(Doc::Hard);
            }
            if pending_lines > 1 {
                docs.push(Doc::Hard);
            }
        } else if let Some(p) = prev {
            let ps = p.text();
            if ps == ";" {
                if node.comment() && pending_lines == 0 {
                    statement.push(text(" "));
                } else if root {
                    if !statement.is_empty() {
                        docs.push(statement_doc(std::mem::take(&mut statement)));
                    }
                    docs.push(Doc::Hard);
                } else {
                    statement.push(Doc::Line(" "));
                }
            } else if ps == "," {
                if node.comment() && pending_lines == 0 {
                    statement.push(text(" "));
                } else {
                    docs.push(statement_doc(std::mem::take(&mut statement)));
                    docs.push(Doc::Line(" "));
                }
            } else if ps == ":" || align && ps == "=" {
                let pad = if align && i >= 2 {
                    key_width.saturating_sub(sig[i - 2].text().chars().count())
                } else {
                    0
                };
                statement.push(text(" ".repeat(1 + pad)));
            } else if s == "." {
                // Chain continuations indent once, regardless of the number of methods.
                if !member_call
                    && sig
                        .get(i + 2)
                        .is_some_and(|n| matches!(n, Node::Group(open, _, _) if open == "("))
                {
                    statement.push(nest(Doc::Line("")));
                }
            } else if !matches!(s, "," | ";" | ":") && ps != "." && !(i >= 1 && unary(&sig, i - 1))
            {
                let adjacent_group = matches!(node, Node::Group(open, _, _) if open == "(" || open == "[")
                    && !operator(ps)
                    && !matches!(ps, "let" | "use" | "as" | "else" | "if");
                if !adjacent_group {
                    statement.push(text(" "));
                }
            }
        }
        let drum = if let Node::Group(open, _, _) = node {
            open == "(" && prev.is_some_and(|p| p.text() == "drums")
        } else {
            false
        };
        let lane_argument = drum_call
            && s == "{"
            && (i == 0
                || i >= 2
                    && sig[i - 2].text() == "lanes"
                    && matches!(sig[i - 1].text(), ":" | "="));
        let doc = node_doc(node, drum || lane_argument);
        // Nest the rest of a method chain so text after a broken dot uses the same indent.
        if member_call {
            statement.push(doc);
        } else if s == "." {
            statement.push(nest(doc));
        } else if prev.is_some_and(|p| p.text() == ".") || (i >= 2 && sig[i - 2].text() == ".") {
            statement.push(nest(doc));
        } else {
            statement.push(doc);
        }
        pending_lines = 0;
        i += 1;
    }
    if !statement.is_empty() {
        docs.push(statement_doc(statement));
    }
    cat(docs)
}

fn node_doc(node: &Node, drum: bool) -> Doc {
    match node {
        Node::Atom(p) => text(&p.text),
        Node::Group(open, nodes, close) => {
            let sig = significant(nodes);
            if sig.is_empty() {
                return text(format!("{open}{close}"));
            }
            // Hug a single collection argument: drums({ ... }), seq([ ... ]).
            if open == "("
                && sig.len() == 1
                && matches!(sig[0], Node::Group(o, _, _) if o == "{" || o == "[")
            {
                return cat(vec![text(open), node_doc(sig[0], drum), text(close)]);
            }
            if open == "("
                && sig.iter().any(|n| n.text() == ",")
                && !sig.iter().any(|n| n.comment())
            {
                let trailing = sig.last().is_some_and(|n| n.text() == ",");
                let args = sig.split(|n| n.text() == ",")
                    .filter(|arg| !arg.is_empty())
                    .map(|arg| {
                        let collection = matches!(arg.last(), Some(Node::Group(o, _, _)) if o == "{" || o == "[");
                        let nodes: Vec<_> = arg.iter().map(|n| (*n).clone()).collect();
                        (sequence(&nodes, false, false, drum), collection)
                    }).collect();
                return Doc::Call(args, trailing);
            }
            let lanes = drum
                && open == "{"
                && sig.iter().filter(|n| matches!(n.text(), ":" | "=")).count() > 1;
            let has_semicolon = sig.iter().any(|n| n.text() == ";");
            let has_colon = sig.iter().any(|n| matches!(n.text(), ":" | "="));
            let block = open == "{"
                && (has_semicolon && !has_colon
                    || sig
                        .first()
                        .is_some_and(|n| matches!(n.text(), "let" | "if" | "fn" | "use")));
            let body = sequence(nodes, block, lanes, drum && open == "(");
            let hard = lanes || block || sig.iter().any(|n| n.comment());
            let line = || if hard { Doc::Hard } else { Doc::Line("") };
            let body = if lanes { force_lines(body) } else { body };
            group(cat(vec![
                text(open),
                nest(cat(vec![line(), body])),
                line(),
                text(close),
            ]))
        }
    }
}
fn force_lines(d: Doc) -> Doc {
    match d {
        Doc::Line(_) => Doc::Hard,
        Doc::Concat(ds) => cat(ds.into_iter().map(force_lines).collect()),
        // Nested values retain their own layout decisions.
        Doc::Group(d) => group(force_lines(*d)),
        d => d,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn stable(source: &str) -> String {
        let out = format(source).unwrap();
        assert_eq!(format(&out).unwrap(), out, "formatter must be idempotent");
        out
    }
    #[test]
    fn drum_lanes_align_without_changing_grids() {
        assert_eq!(
            stable("let beat=drums({kick:\"X...\",snare:\"..x.\",open_hat:\"x.\"});"),
            "let beat = drums({\n    kick:     \"X...\",\n    snare:    \"..x.\",\n    open_hat: \"x.\"\n});\n"
        );
    }
    #[test]
    fn spaces_and_literals() {
        assert_eq!(
            stable("fn f(x=1/2b)=note(38,x,velocity=-0.3).tag(\"a  b\",'x');"),
            "fn f(x = 1/2b) = note(38, x, velocity = -0.3).tag(\"a  b\", 'x');\n"
        );
        stable("let x = 1 / 2; let y = 1/2; let z = !false && -2 < 3;");
    }
    #[test]
    fn comments_and_blank_lines() {
        let out = stable(
            "// intro\n\n\nlet x=1; // trailing\n/* keep\n this */\nlet y=[1,// item\n2];\n",
        );
        for comment in ["// intro", "// trailing", "/* keep\n this */", "// item"] {
            assert!(out.contains(comment));
        }
        assert!(!out.contains("\n\n\n"));
    }
    #[test]
    fn syntax_cases() {
        for source in [
            "fn f(x)=if x > 0 { let y=x+1; y } else { -x };",
            "let x={a=1;b=2};",
            "let x = (1 + 2) * 3;\nlet y = [1,2][0];",
            "let x=1\nlet y=2\nx+y",
            "let x = drums({kick: \"X...\", // kick\nsnare: \"..x.\"});",
            "let x = \"line one\nline two\";",
        ] {
            stable(source);
        }
    }
    #[test]
    fn long_chains_wrap() {
        let out = stable(
            "let material = phrase(\"C4:q D4:q E4:q F4:q\").transpose(-12).velocity(0.6).gate(0.94).humanize(3ms, 0.018, seed=41);",
        );
        assert!(out.contains("\n    .transpose"), "{out}");
    }

    #[test]
    fn calls_attach_expanded_options_records() {
        let out = stable(
            "fx(\"compressor\", {sidechain:\"drums.kick\",threshold_db:-22,ratio:3,attack_ms:3,release_ms:120})",
        );
        assert_eq!(
            out,
            concat!(
                "fx(\"compressor\", {\n",
                "    sidechain: \"drums.kick\",\n",
                "    threshold_db: -22,\n",
                "    ratio: 3,\n",
                "    attack_ms: 3,\n",
                "    release_ms: 120\n",
                "})\n",
            )
        );
    }

    #[test]
    fn multiline_arguments_keep_their_own_closing_line() {
        let out = stable(concat!(
            "track(\"choir\", voices.express({",
            "brightness: [[0, 0.35], [0.65, 0.55], [1, 0.3]],",
            "volume: [[0, 0.65], [0.25, 1], [1, 0.8]]",
            "}), sounds.breath, {gain: -13, sends: {hall: -15}})",
        ));
        assert!(out.contains("\n    voices.express({\n"), "{out}");
        assert!(out.contains("\n    }),\n    sounds.breath, {"), "{out}");
        assert!(!out.contains("voices\n"), "{out}");
        assert!(out.lines().all(|line| line.len() <= WIDTH), "{out}");
    }

    #[test]
    fn calls_pack_arguments_without_exceeding_width() {
        let out = stable(concat!(
            "track(\"piano\",piano_part,",
            "piano(\"default\",{state:\"../presets/bechstein-warm-concert-grand-v2.state\"}),",
            "{gain:-6,strict:true,reach:12,chain:[fx(\"highpass\",{cutoff_hz:70}),",
            "fx(\"eq\",{frequency_hz:270,gain_db:-2,q:0.8})],sends:{hall:-19}})",
        ));
        assert!(
            out.starts_with("track(\"piano\", piano_part,\n    piano("),
            "{out}"
        );
        assert!(out.contains(".state\"}), {\n"), "{out}");
        assert!(out.ends_with("    })\n"), "{out}");
        assert!(out.lines().all(|line| line.len() <= WIDTH), "{out}");
        stable("fx(\"gain\", {gain_db: 3},)");
        let commented = stable("fx(\"gain\", // retain argument comment\n{gain_db: 3})");
        assert!(commented.contains("// retain argument comment\n"));
    }

    #[test]
    fn short_suffix_attaches_to_multiline_receiver() {
        let out = stable(
            "let fill=drums({kick:\"X...\",snare:\"..x.\"}).flam(\"snare\",spread=22ms,grace=0.4);",
        );
        assert!(
            out.contains("}).flam(\"snare\", spread = 22ms, grace = 0.4);\n"),
            "{out}"
        );
        let out = stable("let fill=drums({kick:\"X...\",snare:\"..x.\"}).repeat(2).gain(0.8);");
        assert!(out.contains("}).repeat(2).gain(0.8);"), "{out}");
    }

    #[test]
    fn long_suffix_still_wraps_after_multiline_receiver() {
        let out = stable(
            "let fill=drums({kick:\"X...\",snare:\"..x.\"}).flam(\"snare\",spread=22ms,grace=0.4).humanize(3ms,0.018,seed=41).velocity(0.8).repeat(8);",
        );
        assert!(out.contains("})\n    .flam("), "{out}");
        assert!(out.lines().all(|line| line.len() <= WIDTH));
    }

    #[test]
    fn drum_arguments_and_comments() {
        for source in [
            "drums({kick: \"X...\", snare: \"..x.\"}, span=2bars)",
            "drums(span=2bars, lanes={kick: \"X...\", snare: \"..x.\"})",
        ] {
            let out = stable(source);
            assert!(out.contains("kick:  \"X...\",\n"), "{out}");
            assert!(out.contains("snare: \"..x.\""), "{out}");
        }
        let out = stable("let x=1; // attached\nlet y=[1,2 // last item\n];");
        assert!(out.contains("let x = 1; // attached\n"), "{out}");
        assert!(out.contains("2 // last item\n];"), "{out}");
    }

    #[test]
    fn expanded_lists_keep_short_expressions_compact() {
        let source = format!("seq([{}]);", vec!["m.pulse.repeat(2)"; 12].join(","));
        let out = stable(&source);
        assert_eq!(out.matches("    m.pulse.repeat(2)").count(), 12, "{out}");
        assert!(out.lines().all(|line| line.len() <= WIDTH));
    }

    #[test]
    fn repository_sources_are_stable() {
        fn visit(path: &std::path::Path) {
            for entry in std::fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(&path);
                } else if path.extension().is_some_and(|ext| ext == "muz") {
                    stable(&std::fs::read_to_string(path).unwrap());
                }
            }
        }
        for directory in ["std", "examples", "templates"] {
            visit(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(directory));
        }
    }
}
