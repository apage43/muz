use super::parser::{Shape, Syntax, binary_precedence as precedence};
use anyhow::{Result, ensure};

const WIDTH: usize = 100;

/// Format source without changing tokens, string contents, or comment text.
pub fn format(source: &str) -> Result<String> {
    let syntax = super::parser::parse_for_format(source)?;
    let tokens = super::parser::lex(source)?;
    let mut pieces = Vec::new();
    let mut end = 0;
    for token in tokens.iter().filter(|t| t.at < source.len()) {
        trivia(&source[end..token.at], end, &mut pieces);
        pieces.push(Piece {
            text: source[token.at..token.end].into(),
            kind: if token.string {
                Kind::String
            } else {
                Kind::Code
            },
            lines: 0,
            at: token.at,
            end: token.end,
        });
        end = token.end;
    }
    trivia(&source[end..], end, &mut pieces);
    let comments: Vec<_> = pieces
        .iter()
        .filter(|p| p.kind == Kind::Comment)
        .map(|p| p.text.clone())
        .collect();
    let nodes = tree(&pieces, &mut 0);
    let doc = Formatter { syntax }.sequence(&nodes, true, false, false);
    let mut output = String::new();
    render(&doc, 0, false, &mut output, WIDTH);
    let output = format!("{}\n", output.trim_end_matches('\n'));
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
    let mut after_comments = Vec::new();
    let mut end = 0;
    for token in &after {
        trivia(&output[end..token.at], end, &mut after_comments);
        end = token.end;
    }
    ensure!(
        comments.iter().map(String::as_str).eq(after_comments
            .iter()
            .filter(|p| p.kind == Kind::Comment)
            .map(|p| p.text.as_str())),
        "formatting changed source comments"
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
    at: usize,
    end: usize,
}

// The lexer skips comments; recover them verbatim from the gaps between tokens.
fn trivia(mut s: &str, mut at: usize, out: &mut Vec<Piece>) {
    while !s.is_empty() {
        let comment = s.starts_with("//") || s.starts_with("/*");
        let n = if s.starts_with("//") {
            s.find('\n').unwrap_or(s.len())
        } else if s.starts_with("/*") {
            s.find("*/").unwrap() + 2
        } else {
            s.find(|c: char| !c.is_whitespace()).unwrap_or(s.len())
        };
        let lines = if comment {
            0
        } else {
            s[..n].bytes().filter(|c| *c == b'\n').count()
        };
        if comment || lines > 0 {
            out.push(Piece {
                text: if comment {
                    s[..n].into()
                } else {
                    String::new()
                },
                kind: if comment { Kind::Comment } else { Kind::Break },
                lines,
                at,
                end: at + n,
            });
        }
        s = &s[n..];
        at += n;
    }
}

#[derive(Clone)]
enum Node {
    Atom(Piece),
    Group(Piece, Vec<Node>, Piece),
}
impl Node {
    fn piece(&self) -> &Piece {
        match self {
            Self::Atom(p) | Self::Group(p, _, _) => p,
        }
    }
    fn text(&self) -> &str {
        &self.piece().text
    }
    fn is(&self, text: &str) -> bool {
        self.piece().kind == Kind::Code && self.text() == text
    }
    fn at(&self) -> usize {
        self.piece().at
    }
    fn end(&self) -> usize {
        match self {
            Self::Atom(p) | Self::Group(_, _, p) => p.end,
        }
    }
    fn is_break(&self) -> bool {
        self.piece().kind == Kind::Break
    }
    fn comment(&self) -> bool {
        self.piece().kind == Kind::Comment
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
            let close = pieces[*pos].clone();
            *pos += 1;
            nodes.push(Node::Group(p.clone(), inner, close));
        } else {
            nodes.push(Node::Atom(p.clone()));
        }
    }
    nodes
}

#[derive(Clone)]
enum Doc {
    Text(String),
    LineComment(String),
    Line(&'static str),
    Hard,
    Blank,
    Concat(Vec<Doc>),
    Nest(Box<Doc>),
    Group(Box<Doc>),
    Chain(Box<Doc>, Box<Doc>),
    Attach(Box<Doc>, Box<Doc>),
    Call(Vec<(Doc, bool)>, bool),
    Fill(Vec<Doc>),
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
fn flat_width(d: &Doc) -> Option<usize> {
    match d {
        Doc::Text(s) => (!s.contains('\n')).then(|| s.chars().count()),
        Doc::Line(s) => Some(s.len()),
        Doc::Hard | Doc::Blank | Doc::LineComment(_) => None,
        Doc::Concat(ds) => ds
            .iter()
            .try_fold(0usize, |n, d| n.checked_add(flat_width(d)?)),
        Doc::Nest(d) | Doc::Group(d) => flat_width(d),
        Doc::Chain(receiver, suffix) => flat_width(receiver)?.checked_add(flat_width(suffix)?),
        Doc::Attach(head, body) => flat_width(head)?.checked_add(1 + flat_width(body)?),
        Doc::Call(args, trailing) => args.iter().try_fold(
            2 + args.len().saturating_sub(1) * 2 + usize::from(*trailing),
            |n, (arg, _)| n.checked_add(flat_width(arg)?),
        ),
        Doc::Fill(ds) => ds.iter().try_fold(ds.len().saturating_sub(1), |n, d| {
            n.checked_add(flat_width(d)?)
        }),
    }
}
fn column(out: &str, indent: usize) -> usize {
    if out.is_empty() || out.ends_with('\n') {
        indent
    } else {
        out.rsplit('\n').next().unwrap().chars().count()
    }
}
fn fits(d: &Doc, out: &str, indent: usize, width: usize) -> bool {
    flat_width(d).is_some_and(|n| column(out, indent) + n <= width)
}
fn render(d: &Doc, indent: usize, flat: bool, out: &mut String, width: usize) {
    match d {
        Doc::Text(s) => {
            if out.ends_with('\n') && s.chars().all(|c| c == ' ') {
                return;
            }
            if out.ends_with('\n') {
                out.push_str(&" ".repeat(indent));
            }
            out.push_str(s);
        }
        Doc::LineComment(s) => {
            render(&text(s), indent, false, out, width);
            // Spaces at the end of a comment are part of its original text.
            out.push('\n');
        }
        Doc::Line(s) if flat => {
            if !out.ends_with('\n') {
                out.push_str(s);
            }
        }
        Doc::Line(_) | Doc::Hard => {
            while out.ends_with(' ') {
                out.pop();
            }
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
        Doc::Blank => {
            render(&Doc::Hard, indent, false, out, width);
            if !out.ends_with("\n\n") {
                out.push('\n');
            }
        }
        Doc::Concat(ds) => {
            for (i, d) in ds.iter().enumerate() {
                // Only reserve text before the next potential layout decision.
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
        Doc::Attach(head, body) => {
            render(head, indent, flat, out, width.saturating_sub(1));
            let start = column(out, indent);
            let mut candidate = " ".repeat(start);
            candidate.push(' ');
            render(body, indent, flat, &mut candidate, width);
            let first_line = candidate.lines().next().unwrap_or("").chars().count();
            // Prefer an attached expanding body. Move it only when doing so can
            // repair its first line; indenting an already overlong literal does
            // not help and should not displace its enclosing expression.
            if flat
                || first_line <= width
                || first_line.saturating_sub(start + 1) + indent + 4 > width
            {
                if out.ends_with('\n') {
                    out.push_str(&" ".repeat(indent));
                }
                out.push_str(&candidate[start..]);
            } else {
                render(&Doc::Hard, indent, false, out, width);
                render(body, indent + 4, false, out, width);
            }
        }
        Doc::Group(d) => render(d, indent, flat || fits(d, out, indent, width), out, width),
        Doc::Call(args, trailing) => {
            let inline = flat || fits(d, out, indent, width);
            let expandable: Vec<_> = args
                .iter()
                .enumerate()
                .filter(|(_, (_, hug))| *hug)
                .map(|(i, _)| i)
                .collect();
            // A single collection or callback may expand in place. Everything
            // around it must fit flat, including the complete trailing arguments.
            if !inline && expandable.len() == 1 {
                let expanded = expandable[0];
                let tail = args[expanded + 1..]
                    .iter()
                    .try_fold(1 + usize::from(*trailing), |n, (arg, _)| {
                        Some(n + 2 + flat_width(arg)?)
                    });
                if let Some(tail) = tail {
                    let mut candidate = String::new();
                    candidate.push_str(&" ".repeat(column(out, indent)));
                    candidate.push('(');
                    for (i, (arg, _)) in args.iter().enumerate() {
                        if i > 0 {
                            candidate.push_str(", ");
                        }
                        render(
                            arg,
                            indent,
                            i != expanded,
                            &mut candidate,
                            if i <= expanded {
                                width.saturating_sub(tail)
                            } else {
                                width
                            },
                        );
                    }
                    if *trailing {
                        candidate.push(',');
                    }
                    candidate.push(')');
                    if candidate.lines().all(|line| line.chars().count() <= width) {
                        let start = column(out, indent);
                        if out.ends_with('\n') {
                            out.push_str(&" ".repeat(indent));
                        }
                        out.push_str(&candidate[start..]);
                        return;
                    }
                }
            }
            render(&text("("), indent, flat, out, width);
            for (i, (arg, _)) in args.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if inline {
                    if i > 0 {
                        out.push(' ');
                    }
                } else {
                    render(&Doc::Hard, indent, false, out, width);
                }
                render(
                    arg,
                    indent + if inline { 0 } else { 4 },
                    inline,
                    out,
                    width.saturating_sub(1),
                );
            }
            if *trailing {
                out.push(',');
            }
            if !inline && !args.is_empty() {
                render(&Doc::Hard, indent, false, out, width);
            }
            render(&text(")"), indent, flat, out, width);
        }
        Doc::Chain(receiver, suffix) => {
            let start = out.len();
            render(receiver, indent, flat, out, width);
            let attach = out[start..].contains('\n') && fits(suffix, out, indent, width);
            render(suffix, indent, flat || attach, out, width);
        }
        Doc::Fill(ds) => {
            for (i, item) in ds.iter().enumerate() {
                if i > 0 {
                    if flat
                        || flat_width(item).is_some_and(|n| column(out, indent) + 1 + n <= width)
                    {
                        out.push(' ');
                    } else {
                        render(&Doc::Hard, indent, false, out, width);
                    }
                }
                render(item, indent, flat, out, width);
            }
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
    (nodes[i].is("-") || nodes[i].is("!"))
        && (i == 0
            || nodes[i - 1].piece().kind == Kind::Code && operator(nodes[i - 1].text())
            || matches!(nodes[i - 1].text(), "," | ":" | ";"))
}
fn trim(nodes: &[Node]) -> &[Node] {
    let start = nodes
        .iter()
        .position(|n| !n.is_break())
        .unwrap_or(nodes.len());
    let end = nodes
        .iter()
        .rposition(|n| !n.is_break())
        .map_or(start, |i| i + 1);
    &nodes[start..end]
}
struct Formatter {
    syntax: Syntax,
}
impl Formatter {
    fn shape(&self, nodes: &[Node]) -> Option<Shape> {
        let nodes = trim(nodes);
        self.syntax
            .expressions
            .get(&(nodes.first()?.at(), nodes.last()?.end()))
            .copied()
    }
    fn expression(&self, nodes: &[Node], drum: bool) -> Doc {
        let nodes = trim(nodes);
        if nodes.is_empty() {
            return text("");
        }
        let Some(first) = nodes.iter().position(|n| !n.comment() && !n.is_break()) else {
            return self.inline(nodes, drum);
        };
        if first > 0 {
            let separated = nodes[..first].iter().any(Node::is_break);
            return cat(vec![
                self.inline(&nodes[..first], drum),
                if separated { Doc::Hard } else { text(" ") },
                self.expression(&nodes[first..], drum),
            ]);
        }
        let last = nodes
            .iter()
            .rposition(|n| !n.comment() && !n.is_break())
            .unwrap();
        if last + 1 < nodes.len() {
            let gap = nodes[last + 1..]
                .iter()
                .take_while(|n| n.is_break())
                .map(|n| n.piece().lines)
                .max()
                .unwrap_or(0);
            return cat(vec![
                self.expression(&nodes[..=last], drum),
                match gap {
                    0 => text(" "),
                    1 => Doc::Hard,
                    _ => Doc::Blank,
                },
                self.inline(&nodes[last + 1..], drum),
            ]);
        }
        // Punctuation and trailing comments belong to the expression's final line.
        if nodes.last().is_some_and(|n| n.is(";") || n.is(",")) {
            return cat(vec![
                self.expression(&nodes[..nodes.len() - 1], drum),
                text(nodes.last().unwrap().text()),
            ]);
        }
        if let Some(eq) = nodes
            .iter()
            .position(|n| n.is("=") && self.syntax.definitions.contains(&n.at()))
        {
            let head = self.inline(&nodes[..=eq], false);
            let body = trim(&nodes[eq + 1..]);
            return self.binding(head, body, drum);
        }
        match self.shape(nodes) {
            Some(Shape::Binary { operator: at }) => {
                let i = nodes.iter().position(|n| n.at() == at).unwrap();
                // Flatten the left spine at equal precedence. A whole logical
                // clause stays grouped while lower-precedence operators break.
                let mut parts = Vec::new();
                self.binary_parts(nodes, precedence(nodes[i].text()), drum, &mut parts);
                group(cat(parts))
            }
            Some(Shape::Lambda { arrow }) => {
                let i = nodes.iter().position(|n| n.at() == arrow).unwrap();
                self.binding(self.inline(&nodes[..=i], false), &nodes[i + 1..], drum)
            }
            Some(Shape::If { yes, no }) => {
                let yes = nodes.iter().position(|n| n.at() == yes).unwrap();
                let no = nodes.iter().position(|n| n.at() == no).unwrap();
                group(cat(vec![
                    text("if "),
                    self.expression(&nodes[1..yes], false),
                    text(" "),
                    self.node_doc(&nodes[yes], false, true),
                    text(" "),
                    self.inline(&nodes[yes + 1..no], false),
                    text(" "),
                    self.node_doc(&nodes[no], false, true),
                ]))
            }
            None => self.inline(nodes, drum),
        }
    }
    fn binding(&self, head: Doc, body: &[Node], drum: bool) -> Doc {
        let body = trim(body);
        if body.last().is_some_and(|n| n.is(",") || n.is(";")) {
            return cat(vec![
                self.binding(head, &body[..body.len() - 1], drum),
                text(body.last().unwrap().text()),
            ]);
        }
        if let Some(Shape::Binary { operator }) = self.shape(body) {
            let op = body.iter().find(|n| n.at() == operator).unwrap();
            let mut parts = Vec::new();
            self.binary_parts(body, precedence(op.text()), drum, &mut parts);
            group(cat(vec![head, nest(cat(vec![Doc::Line(" "), cat(parts)]))]))
        } else {
            Doc::Attach(Box::new(head), Box::new(self.expression(body, drum)))
        }
    }
    fn binary_parts(&self, nodes: &[Node], prec: u8, drum: bool, docs: &mut Vec<Doc>) {
        let nodes = trim(nodes);
        if let Some(Shape::Binary { operator: at }) = self.shape(nodes) {
            let i = nodes.iter().position(|n| n.at() == at).unwrap();
            if precedence(nodes[i].text()) == prec {
                self.binary_parts(&nodes[..i], prec, drum, docs);
                docs.push(Doc::Line(" "));
                docs.push(text(nodes[i].text()));
                docs.push(text(" "));
                docs.push(self.expression(&nodes[i + 1..], drum));
                return;
            }
        }
        docs.push(self.expression(nodes, drum));
    }
    // Split statements/items before formatting their expressions. Trivia remains
    // in each slice so end-of-line comments keep their original attachment.
    fn sequence(&self, nodes: &[Node], root: bool, align: bool, drum: bool) -> Doc {
        let mut docs = Vec::new();
        let mut start = 0;
        let mut pending = 0;
        let mut previous: Option<&Node> = None;
        for (i, node) in nodes.iter().enumerate() {
            if node.is_break() {
                pending = pending.max(node.piece().lines);
                continue;
            }
            let boundary = previous.is_some_and(|p| {
                (p.is(",") || p.is(";")) && !(node.comment() && pending == 0)
                    || root && self.syntax.statements.contains(&node.at())
                    || p.comment()
                        && pending > 0
                        && nodes[start..i].iter().any(|n| n.is(",") || n.is(";"))
            });
            if boundary {
                docs.push(self.item(&nodes[start..i], align, drum, nodes));
                docs.push(
                    if root || pending > 1 || previous.unwrap().comment() || node.comment() {
                        Doc::Hard
                    } else {
                        Doc::Line(" ")
                    },
                );
                if pending > 1 {
                    docs.push(Doc::Blank);
                }
                start = i;
            }
            pending = 0;
            previous = Some(node);
        }
        docs.push(self.item(&nodes[start..], align, drum, nodes));
        cat(docs)
    }
    fn item(&self, nodes: &[Node], align: bool, drum: bool, siblings: &[Node]) -> Doc {
        let nodes = trim(nodes);
        if let Some(i) = nodes
            .iter()
            .position(|n| n.is(":") || n.is("=") && !self.syntax.definitions.contains(&n.at()))
        {
            let pad = if align {
                let sig = significant(siblings);
                let max = sig
                    .windows(2)
                    .filter(|w| w[1].is(":") || w[1].is("="))
                    .map(|w| w[0].text().chars().count())
                    .max()
                    .unwrap_or(0);
                max.saturating_sub(nodes.first().map_or(0, |n| n.text().chars().count()))
            } else {
                0
            };
            return self.binding(
                cat(vec![
                    self.inline(&nodes[..=i], false),
                    text(" ".repeat(pad)),
                ]),
                &nodes[i + 1..],
                drum,
            );
        }
        self.expression(nodes, drum)
    }
    fn inline(&self, nodes: &[Node], drum: bool) -> Doc {
        let sig = significant(nodes);
        let methods: Vec<_> = sig
            .iter()
            .enumerate()
            .filter(|(i, n)| n.is(".") && sig.get(i + 2).is_some_and(|n| n.is("(")))
            .map(|(i, _)| i)
            .collect();
        let chain = methods.len() > 1;
        let mut docs = Vec::new();
        let mut suffix = Vec::new();
        let mut chained = false;
        let mut pending = 0;
        let mut i: usize = 0;
        for node in nodes {
            if node.is_break() {
                pending = pending.max(node.piece().lines);
                continue;
            }
            let prev = i.checked_sub(1).map(|j| sig[j]);
            if chain && methods.contains(&i) {
                chained = true;
            }
            let target = if chained { &mut suffix } else { &mut docs };
            if let Some(p) = prev {
                if p.comment() && p.text().starts_with("//") || node.comment() && pending > 0 {
                    target.push(Doc::Hard);
                    if pending > 1 {
                        target.push(Doc::Blank);
                    }
                } else if chain && methods.contains(&i) {
                    target.push(Doc::Line(""));
                } else if !node.is(".")
                    && !p.is(".")
                    && !node.is(",")
                    && !node.is(";")
                    && !node.is(":")
                    && !unary(&sig, i - 1)
                {
                    let adjacent = (node.is("(") || node.is("["))
                        && !operator(p.text())
                        && !matches!(p.text(), "let" | "use" | "as" | "else" | "if")
                        && !p.comment();
                    if !adjacent {
                        target.push(text(" "));
                    }
                }
            }
            let lanes = drum && node.is("{") && (i == 0 || i >= 2 && sig[i - 2].is("lanes"));
            let drum_call = node.is("(") && prev.is_some_and(|n| n.is("drums"));
            target.push(self.node_doc(node, lanes || drum_call, false));
            pending = 0;
            i += 1;
        }
        if chain {
            group(Doc::Chain(Box::new(cat(docs)), Box::new(nest(cat(suffix)))))
        } else {
            group(cat(docs))
        }
    }
    fn node_doc(&self, node: &Node, drum: bool, conditional: bool) -> Doc {
        let Node::Group(open, nodes, close) = node else {
            return if node.comment() && node.text().starts_with("//") {
                Doc::LineComment(node.text().into())
            } else {
                text(node.text())
            };
        };
        let sig = significant(nodes);
        if sig.is_empty() {
            return text(format!("{}{}", open.text, close.text));
        }
        let commented = sig.iter().any(|n| n.comment());
        let blank = nodes.iter().any(|n| n.is_break() && n.piece().lines > 1);
        let block = self.syntax.blocks.contains(&open.at);
        if open.text == "(" && self.syntax.calls.contains(&open.at) && !commented && !blank {
            let trailing = sig.last().is_some_and(|n| n.is(","));
            let args = nodes
                .split(|n| n.is(","))
                .map(trim)
                .filter(|ns| !ns.is_empty())
                .map(|ns| {
                    let value = ns
                        .iter()
                        .position(|n| n.is("=") || n.is(":"))
                        .map_or(ns, |i| trim(&ns[i + 1..]));
                    let hug = value.len() == 1 && (value[0].is("{") || value[0].is("["))
                        || matches!(self.shape(value), Some(Shape::Lambda { .. }))
                            && value.last().is_some_and(|n| n.is("{") || n.is("["));
                    (self.item(ns, false, drum, nodes), hug)
                })
                .collect();
            return Doc::Call(args, trailing);
        }
        let lanes =
            drum && open.text == "{" && sig.iter().filter(|n| n.is(":") || n.is("=")).count() > 1;
        let statements = sig
            .iter()
            .filter(|n| self.syntax.statements.contains(&n.at()))
            .count();
        let hard = lanes || commented || blank || block && statements > 1;
        let body = if open.text == "["
            && !commented
            && !blank
            && sig
                .iter()
                .all(|n| matches!(n, Node::Atom(_)) && (!operator(n.text()) || n.is("-")))
        {
            let items = nodes
                .split_inclusive(|n| n.is(","))
                .map(trim)
                .filter(|ns| !ns.is_empty())
                .map(|ns| self.expression(ns, false))
                .collect();
            Doc::Fill(items)
        } else {
            self.sequence(nodes, block, lanes, drum && open.text == "(")
        };
        let line = || {
            if hard {
                Doc::Hard
            } else {
                Doc::Line(if block { " " } else { "" })
            }
        };
        let body = if lanes { force_lines(body) } else { body };
        let doc = cat(vec![
            text(&open.text),
            nest(cat(vec![line(), body])),
            line(),
            text(&close.text),
        ]);
        if conditional { doc } else { group(doc) }
    }
}
fn force_lines(d: Doc) -> Doc {
    match d {
        Doc::Line(_) => Doc::Hard,
        Doc::Concat(ds) => cat(ds.into_iter().map(force_lines).collect()),
        // Nested values retain their own layout decisions.
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
        assert!(
            out.contains("\n    }),\n    sounds.breath,\n    {"),
            "{out}"
        );
        assert!(out.ends_with("\n)\n"), "{out}");
        assert!(!out.contains("voices\n"), "{out}");
        assert!(out.lines().all(|line| line.len() <= WIDTH), "{out}");
    }

    #[test]
    fn complex_calls_use_vertical_arguments_without_exceeding_width() {
        let out = stable(concat!(
            "track(\"piano\",piano_part,",
            "piano(\"default\",{state:\"../presets/bechstein-warm-concert-grand-v2.state\"}),",
            "{gain:-6,strict:true,reach:12,chain:[fx(\"highpass\",{cutoff_hz:70}),",
            "fx(\"eq\",{frequency_hz:270,gain_db:-2,q:0.8})],sends:{hall:-19}})",
        ));
        assert!(
            out.starts_with("track(\n    \"piano\",\n    piano_part,\n    piano("),
            "{out}"
        );
        assert!(out.contains(".state\"}),\n    {\n"), "{out}");
        assert!(out.ends_with("    }\n)\n"), "{out}");
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
    fn definitions_break_between_logical_clauses() {
        assert_eq!(
            stable(
                "fn at_chorus(at) = (at >= 96b && at < 160b) || (at >= 256b && at < 320b) || (at >= 384b && at < 464b);"
            ),
            concat!(
                "fn at_chorus(at) =\n",
                "    (at >= 96b && at < 160b)\n",
                "    || (at >= 256b && at < 320b)\n",
                "    || (at >= 384b && at < 464b);\n",
            )
        );
        let out = stable(
            "fn choose_level(velocity, phrase, section, articulation) = velocity + phrase + section + articulation + velocity * phrase + section * articulation;",
        );
        assert!(
            out.starts_with("fn choose_level(velocity, phrase, section, articulation) =\n"),
            "{out}"
        );
        assert!(out.contains("\n    + velocity * phrase\n"), "{out}");
        assert!(out.lines().all(|line| line.len() <= WIDTH), "{out}");
    }

    #[test]
    fn arithmetic_wraps_before_splitting_small_calls() {
        assert_eq!(
            stable(
                "fn level(n) = n.velocity * phrase_level(n.at) * section_level(n.at) * dynamic_weight(n.pitch) + articulation_boost(n.tags);"
            ),
            concat!(
                "fn level(n) =\n",
                "    n.velocity * phrase_level(n.at) * section_level(n.at) * dynamic_weight(n.pitch)\n",
                "    + articulation_boost(n.tags);\n",
            )
        );
        stable("let value = 1 / 2 - -3 * (4 + 5); let unit = 1/2b; let negative = -1/2b;");
    }

    #[test]
    fn long_simple_values_and_expression_callbacks_have_continuations() {
        let name = "reference".repeat(7);
        let binding = "binding".repeat(7);
        assert_eq!(
            stable(&format!("let {binding} = {name};")),
            format!("let {binding} =\n    {name};\n")
        );
        let out = stable(
            "map(pattern, fn(n) => n.velocity * phrase_level(n.at) * section_level(n.at) * dynamic_weight(n.pitch) + articulation_boost(n.tags));",
        );
        assert!(
            out.starts_with("map(\n    pattern,\n    fn(n) =>\n"),
            "{out}"
        );
        assert!(
            out.contains("\n        + articulation_boost(n.tags)\n"),
            "{out}"
        );
        assert!(out.lines().all(|line| line.len() <= WIDTH), "{out}");
        let out = stable(
            "let x = {velocity: n.velocity * phrase_level(n.at) * section_level(n.at) * dynamic_weight(n.pitch) + articulation_boost(n.tags), gate: 0.9};",
        );
        assert!(out.contains("\n    velocity:\n"), "{out}");
        assert!(
            out.contains("\n        + articulation_boost(n.tags),\n"),
            "{out}"
        );
    }

    #[test]
    fn callbacks_keep_their_introduction_attached() {
        assert_eq!(
            stable(
                "fn shape(p) = p.map_notes(fn(n) => {gate: if n.duration < 1b {0.85} else {0.98}, velocity: clamp(n.velocity + phrase_level(n.at))});"
            ),
            concat!(
                "fn shape(p) = p.map_notes(fn(n) => {\n",
                "    gate: if n.duration < 1b { 0.85 } else { 0.98 },\n",
                "    velocity: clamp(n.velocity + phrase_level(n.at))\n",
                "});\n",
            )
        );
        assert_eq!(
            stable(
                "fn shape(p) = p.map_notes(fn(n) => {let v=n.velocity; {velocity:v,gate:0.9}});"
            ),
            concat!(
                "fn shape(p) = p.map_notes(fn(n) => {\n",
                "    let v = n.velocity;\n",
                "    {velocity: v, gate: 0.9}\n",
                "});\n",
            )
        );
    }

    #[test]
    fn expanded_collections_attach_short_trailing_arguments() {
        assert_eq!(
            stable(
                "curve([[0b, 0], [4b, -0.5], [8b, -1], [12b, 0.4], [16b, 0.7], [20b, 0], [24b, -0.7], [28b, 0.4], [32b, 0]], \"smooth\");"
            ),
            concat!(
                "curve([\n",
                "    [0b, 0],\n    [4b, -0.5],\n    [8b, -1],\n    [12b, 0.4],\n",
                "    [16b, 0.7],\n    [20b, 0],\n    [24b, -0.7],\n    [28b, 0.4],\n    [32b, 0]\n",
                "], \"smooth\");\n",
            )
        );
        assert_eq!(
            stable("curve([1, // keep\n2], \"smooth\",)"),
            "curve([\n    1, // keep\n    2\n], \"smooth\",)\n"
        );
        let out = stable("map(items, fn(n) => {let x=n+1; x}, selector=\"all\");");
        assert!(out.starts_with("map(items, fn(n) => {\n"), "{out}");
        assert!(out.ends_with("}, selector = \"all\");\n"), "{out}");
    }

    #[test]
    fn conditionals_expand_both_branches_and_space_short_blocks() {
        assert_eq!(
            stable("fn choose(x) = if x > 0 {1} else {-1};"),
            "fn choose(x) = if x > 0 { 1 } else { -1 };\n"
        );
        assert_eq!(
            stable("fn choose(x) = if x > 0 {let y=x+1;y} else {-x};"),
            "fn choose(x) = if x > 0 {\n    let y = x + 1;\n    y\n} else {\n    -x\n};\n"
        );
        assert_eq!(
            stable("if x { // why\n1} else {2}"),
            "if x {\n    // why\n    1\n} else {\n    2\n}\n"
        );
        assert_eq!(
            stable("let x = {value:1}; fn value() {1}"),
            "let x = {value: 1};\nfn value() { 1 }\n"
        );
        stable("if x {1} /* between */ else /* body */ {2}");
    }

    #[test]
    fn collections_preserve_blank_line_groups_and_comment_attachment() {
        assert_eq!(
            stable("let form = [verse_a, verse_b,\n\n\nchorus_a, chorus_b];"),
            "let form = [\n    verse_a,\n    verse_b,\n\n    chorus_a,\n    chorus_b\n];\n"
        );
        assert_eq!(
            stable("let mix = {gain:-3,\n\n// room\nsends:{hall:-12},pan:0};"),
            "let mix = {\n    gain: -3,\n\n    // room\n    sends: {hall: -12},\n    pan: 0\n};\n"
        );
        assert_eq!(
            stable("let xs = [one, // one\n\n// two\ntwo, three];"),
            "let xs = [\n    one, // one\n\n    // two\n    two,\n    three\n];\n"
        );
    }

    #[test]
    fn scalar_arrays_pack_rows_but_structural_items_keep_their_rows() {
        let out = stable(&format!("let steps = [{}];", vec!["12345"; 30].join(",")));
        assert!(out.lines().count() < 10, "{out}");
        assert!(out.contains("    12345, 12345, 12345,"), "{out}");
        assert!(out.lines().all(|line| line.len() <= WIDTH), "{out}");
        let out = stable(&format!(
            "let points = [{}];",
            vec!["[12345, 67890]"; 12].join(",")
        ));
        assert_eq!(
            out.lines().filter(|line| line.starts_with("    [")).count(),
            12,
            "{out}"
        );
    }

    #[test]
    fn comments_between_operator_clauses_keep_continuation_indent() {
        assert_eq!(
            stable(
                "fn at_chorus(at) = (at >= 96b && at < 160b) || // first\n(at >= 256b && at < 320b) || (at >= 384b && at < 464b);"
            ),
            concat!(
                "fn at_chorus(at) =\n",
                "    (at >= 96b && at < 160b)\n",
                "    || // first\n",
                "    (at >= 256b && at < 320b)\n",
                "    || (at >= 384b && at < 464b);\n"
            )
        );
        for source in [
            "let x = a // left\n+ b;",
            "let x = a + /* right */ b;",
            "let x = a + // right\nb;",
            "let x = 1 // suffix\n;",
            "call(one, // first\ntwo, three)",
            "call(// first\none, two)",
        ] {
            stable(source);
        }
    }

    #[test]
    fn punctuation_inside_strings_does_not_affect_layout() {
        for source in [
            "let xs=[\",\",\";\",\"=\",\"+\",\"//\"];",
            "f(\",\",\"=\",\"else\");",
            "let x={\"=\":1,\",\":2};",
            "let x = \"line one\nline two\"; // untouched",
        ] {
            stable(source);
        }
    }

    #[test]
    fn comment_text_including_trailing_spaces_is_preserved() {
        assert_eq!(
            stable("let x=1; // keep two spaces  \n"),
            "let x = 1; // keep two spaces  \n"
        );
        assert_eq!(stable("// keep two spaces  "), "// keep two spaces  \n");
        stable("let xs=[1, // keep two spaces  \n2]; /* keep\n spacing  */");
    }

    #[test]
    fn nested_layouts_reserve_closing_delimiters_and_trailing_arguments() {
        for length in 50..110 {
            let name = "n".repeat(length);
            for source in [
                format!("let value = outer(inner({{key: [{name}, 1, 2]}}), tail);"),
                format!("fn value(x) = x.map(fn(n) => {{value: n + {name}}}).gain(0.8);"),
                format!("curve([1,2,3,4,5,6,7,8], mode={name}, wet=0.5);"),
            ] {
                let out = stable(&source);
                // Identifiers are indivisible; these cases through 80 columns
                // all have enough room for their syntax and indentation.
                if length <= 80 {
                    assert!(out.lines().all(|line| line.len() <= WIDTH), "{out}");
                }
            }
        }
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
