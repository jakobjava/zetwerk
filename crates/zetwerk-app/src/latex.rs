//! LaTeX habits in equations: what somebody who has written mathematics
//! in LaTeX for years types, pastes or asks for, as the Typst that means
//! the same.
//!
//! - [`to_typst`] reads LaTeX mathematics (`\frac{\partial u}{\partial t}`)
//!   and writes Typst's (`(partial u)/(partial t)`). Whatever it is given,
//!   what it writes is read by Typst: a command it does not know is kept
//!   as text (`"\foo"`) and named in [`Converted::kept`].
//! - [`markup_to_typst`] does that to the mathematics in running text
//!   (`$…$`, `\(…\)`, `\[…\]`, `$$…$$`, `\begin{equation}…`), for what is
//!   pasted there.
//! - [`to_latex`] goes the other way, for copying an equation into
//!   another program.
//! - [`typing`] is what keys typed in an equation do for a LaTeX writer:
//!   a backslash begins a name, shown as typed until it is over.
//!
//! The names are in `table`; Typst's are checked against Typst by the
//! tests, not trusted.

use std::fmt::Write;
use std::sync::atomic::{AtomicBool, Ordering};

mod back;
mod table;
pub mod typing;

pub use back::{symbol_char, to_latex};
pub use table::{COMMANDS, SYMBOLS, Shape, command, symbol};

/// Whether what is typed in an equation is read as LaTeX, as the
/// settings say (`[typing] latex`). Pasting and `:latex` convert all the
/// same: they are asked for.
static TYPING: AtomicBool = AtomicBool::new(true);

/// Says whether typing in equations reads LaTeX.
pub fn set_typing(on: bool) {
    TYPING.store(on, Ordering::Relaxed);
}

/// Whether typing in equations reads LaTeX.
#[must_use]
pub fn typing_on() -> bool {
    TYPING.load(Ordering::Relaxed)
}

/// Mathematics written the other way, and what of it could not be.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Converted {
    pub text: String,
    /// What was kept as it was written, as text: commands and
    /// environments nobody here knows (`\foo`), each once.
    pub kept: Vec<String>,
}

impl Converted {
    fn keep(kept: &mut Vec<String>, what: String) {
        if !kept.contains(&what) {
            kept.push(what);
        }
    }
}

/// Whether `text`, put into an equation, looks like LaTeX rather than
/// Typst: a backslash before a letter (that is no Typst escape such as
/// `\u{…}`), or braces behind `^` and `_`.
#[must_use]
pub fn looks_like_latex(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.windows(2).enumerate().any(|(at, pair)| match pair {
        [b'\\', c] if c.is_ascii_alphabetic() => !(*c == b'u' && bytes.get(at + 2) == Some(&b'{')),
        [b'\\', b'\\' | b',' | b';' | b'!' | b'{' | b'}'] | [b'^' | b'_', b'{'] => true,
        _ => false,
    })
}

/// LaTeX mathematics (what stands between dollar signs) as Typst's.
#[must_use]
pub fn to_typst(latex: &str) -> Converted {
    let mut parser = Parser::new(lex(latex));
    let nodes = parser.sequence(Until::End);
    Converted {
        text: print_all(&nodes, false),
        kept: parser.kept,
    }
}

/// The mathematics of LaTeX in running text as Typst's equations: `$…$`
/// and `\(…\)` inline, `$$…$$`, `\[…\]` and the environments of displayed
/// mathematics standing by themselves. The rest stays as it is. `None`
/// where nothing in `text` is LaTeX: a `$x$` alone is as much Typst.
#[must_use]
pub fn markup_to_typst(text: &str) -> Option<Converted> {
    let mut out = String::with_capacity(text.len());
    let mut kept = Vec::new();
    let mut latex = false;
    let mut at = 0;
    while at < text.len() {
        let rest = &text[at..];
        let found = math_at(rest);
        let Some((length, inner, display)) = found else {
            let c = rest.chars().next().map_or(1, char::len_utf8);
            // An escaped sign is the sign: `\$` is no equation.
            let step = if rest.starts_with('\\') {
                c + rest[c..].chars().next().map_or(0, char::len_utf8)
            } else {
                c
            };
            out.push_str(&rest[..step]);
            at += step;
            continue;
        };
        latex |= !rest.starts_with('$') || rest.starts_with("$$") || looks_like_latex(inner);
        let converted = to_typst(inner);
        for what in converted.kept {
            Converted::keep(&mut kept, what);
        }
        let body = converted.text.trim().to_owned();
        if display {
            let _ = write!(out, "$ {body} $");
        } else if body.is_empty() {
            out.push_str("$\"\"$");
        } else {
            let _ = write!(out, "${body}$");
        }
        at += length;
    }
    latex.then_some(Converted { text: out, kept })
}

/// The mathematics `text` begins with, in one of LaTeX's ways of writing
/// it: how long it is, what is between its marks, and whether it is
/// displayed.
fn math_at(text: &str) -> Option<(usize, &str, bool)> {
    let between = |open: &str, close: &str, display: bool| {
        let inner = text.strip_prefix(open)?;
        let end = find_unescaped(inner, close)?;
        Some((open.len() + end + close.len(), &inner[..end], display))
    };
    if text.starts_with("$$") {
        return between("$$", "$$", true);
    }
    if text.starts_with('$') {
        return between("$", "$", false);
    }
    if text.starts_with("\\(") {
        return between("\\(", "\\)", false);
    }
    if text.starts_with("\\[") {
        return between("\\[", "\\]", true);
    }
    let name = text.strip_prefix("\\begin{")?;
    let name = &name[..name.find('}')?];
    let displayed = [
        "equation",
        "equation*",
        "align",
        "align*",
        "gather",
        "gather*",
        "multline",
        "multline*",
        "displaymath",
        "eqnarray",
        "eqnarray*",
        "flalign",
        "flalign*",
    ];
    if !displayed.contains(&name) {
        return None;
    }
    let close = format!("\\end{{{name}}}");
    let end = text.find(&close)?;
    Some((end + close.len(), &text[..end + close.len()], true))
}

/// Where `close` first stands in `text` other than right behind a
/// backslash.
fn find_unescaped(text: &str, close: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(found) = text[from..].find(close) {
        let at = from + found;
        let backslashes = text[..at].len() - text[..at].trim_end_matches('\\').len();
        if backslashes.is_multiple_of(2) {
            return Some(at);
        }
        from = at + close.len();
    }
    None
}

// ---- Reading LaTeX ---------------------------------------------------

/// A piece of LaTeX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    /// A backslash and the name behind it (letters, or one other
    /// character; nothing where the text ends with the backslash).
    Command(&'a str),
    Open,
    Close,
    Sup,
    Sub,
    Amp,
    Primes(usize),
    Blank,
    Char(char),
}

fn lex(text: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut at = 0;
    while let Some(c) = text[at..].chars().next() {
        let start = at;
        at += c.len_utf8();
        let token = match c {
            '\\' => {
                let rest = &text[at..];
                let letters = rest.len()
                    - rest
                        .trim_start_matches(|c: char| c.is_ascii_alphabetic())
                        .len();
                let length = if letters > 0 {
                    letters
                } else {
                    rest.chars().next().map_or(0, char::len_utf8)
                };
                at += length;
                Token::Command(&rest[..length])
            }
            '{' => Token::Open,
            '}' => Token::Close,
            '^' => Token::Sup,
            '_' => Token::Sub,
            '&' => Token::Amp,
            '\'' => {
                let primes = text[start..].len() - text[start..].trim_start_matches('\'').len();
                at = start + primes;
                Token::Primes(primes)
            }
            '%' => {
                at = text[at..].find('\n').map_or(text.len(), |end| at + end);
                continue;
            }
            c if c.is_whitespace() => {
                at = text.len() - text[at..].trim_start().len();
                Token::Blank
            }
            c => Token::Char(c),
        };
        tokens.push(token);
    }
    tokens
}

/// What LaTeX mathematics is made of, as far as Typst needs to know.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    /// A letter, a number, the name of a symbol, an escaped sign.
    Atom(String),
    /// An operator or a mark of punctuation.
    Op(String),
    /// What braces group, which shows as nothing.
    Seq(Vec<Node>),
    Attach {
        base: Box<Node>,
        primes: usize,
        sub: Option<Box<Node>>,
        sup: Option<Box<Node>>,
    },
    Frac(Box<Node>, Box<Node>),
    Call {
        name: String,
        args: Vec<Node>,
        named: Vec<String>,
    },
    Text(String),
    /// Delimiters made as large as what is between them.
    Lr(&'static str, Vec<Node>, &'static str),
    Grid(table::Layout, Vec<Vec<Vec<Node>>>),
    Break,
    Align,
    Space(&'static str),
    /// A command nobody here knows: kept as text.
    Kept(String),
}

fn empty() -> Node {
    Node::Seq(Vec::new())
}

/// Where a sequence ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Until {
    End,
    Close,
    Right,
    Cell,
    Bracket,
}

/// How deep groups may be nested before what is inside is kept as text:
/// a stack is no place for a thousand braces.
const DEEPEST: usize = 64;

struct Parser<'a> {
    tokens: Vec<Token<'a>>,
    at: usize,
    kept: Vec<String>,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn new(tokens: Vec<Token<'a>>) -> Self {
        Self {
            tokens,
            at: 0,
            kept: Vec::new(),
            depth: 0,
        }
    }

    fn peek(&self) -> Option<Token<'a>> {
        self.tokens.get(self.at).copied()
    }

    fn next(&mut self) -> Option<Token<'a>> {
        let token = self.peek();
        self.at += 1;
        token
    }

    fn skip_blanks(&mut self) {
        while self.peek() == Some(Token::Blank) {
            self.at += 1;
        }
    }

    fn stops(&self, until: Until) -> bool {
        let Some(token) = self.peek() else {
            return true;
        };
        match until {
            Until::End => false,
            Until::Close => token == Token::Close,
            Until::Right => matches!(token, Token::Close | Token::Command("right")),
            Until::Cell => matches!(
                token,
                Token::Close | Token::Amp | Token::Command("\\" | "cr" | "end")
            ),
            Until::Bracket => matches!(token, Token::Close | Token::Char(']')),
        }
    }

    /// What stands up to where `until` says it ends, which is not taken.
    fn sequence(&mut self, until: Until) -> Vec<Node> {
        let mut items = Vec::new();
        // `\over` and `\choose` part what is before them from what is
        // behind.
        let mut parted: Option<(usize, bool)> = None;
        loop {
            self.skip_blanks();
            if self.stops(until) {
                break;
            }
            match self.peek() {
                // A brace closing nothing.
                Some(Token::Close) => {
                    self.at += 1;
                    continue;
                }
                Some(Token::Command(over @ ("over" | "choose" | "atop"))) => {
                    self.at += 1;
                    parted = Some((items.len(), over == "choose"));
                    continue;
                }
                _ => {}
            }
            let before = self.at;
            if let Some(item) = self.item(until) {
                items.push(item);
            }
            if self.at == before {
                self.at += 1;
            }
        }
        match parted {
            Some((at, choose)) => {
                let below = items.split_off(at);
                let (above, below) = (Node::Seq(items), Node::Seq(below));
                vec![if choose {
                    call("binom", vec![above, below])
                } else {
                    Node::Frac(Box::new(above), Box::new(below))
                }]
            }
            None => items,
        }
    }

    /// What is inside braces, the `{` taken: up to the `}`, which is
    /// taken too.
    fn group(&mut self) -> Vec<Node> {
        if self.depth >= DEEPEST {
            return vec![Node::Text(self.raw_group())];
        }
        self.depth += 1;
        let items = self.sequence(Until::Close);
        self.depth -= 1;
        if self.peek() == Some(Token::Close) {
            self.at += 1;
        }
        items
    }

    /// The tokens of a group up to its `}` as text, the `{` taken.
    fn raw_group(&mut self) -> String {
        let mut depth = 1;
        let mut text = String::new();
        while let Some(token) = self.next() {
            match token {
                Token::Open => depth += 1,
                Token::Close => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            text.push_str(&written(token));
        }
        text
    }

    fn item(&mut self, until: Until) -> Option<Node> {
        let base = self.atom(until)?;
        Some(self.scripts(base))
    }

    /// What is raised, lowered and primed behind `base`.
    fn scripts(&mut self, base: Node) -> Node {
        let (mut primes, mut sub, mut sup) = (0, None, None);
        loop {
            self.skip_blanks();
            match self.peek() {
                Some(Token::Primes(count)) if sup.is_none() => {
                    self.at += 1;
                    primes += count;
                }
                Some(Token::Sup) if sup.is_none() => {
                    self.at += 1;
                    sup = Some(Box::new(self.argument()));
                }
                Some(Token::Sub) if sub.is_none() => {
                    self.at += 1;
                    sub = Some(Box::new(self.argument()));
                }
                _ => break,
            }
        }
        if primes == 0 && sub.is_none() && sup.is_none() {
            return base;
        }
        Node::Attach {
            base: Box::new(base),
            primes,
            sub,
            sup,
        }
    }

    fn atom(&mut self, until: Until) -> Option<Node> {
        match self.next()? {
            Token::Blank | Token::Close => None,
            Token::Open => Some(Node::Seq(self.group())),
            Token::Sup | Token::Sub | Token::Primes(_) => {
                self.at -= 1;
                Some(self.scripts(empty()))
            }
            Token::Amp => Some(Node::Align),
            Token::Char(c) => self.character(c, false),
            Token::Command(name) => self.command(name, until),
        }
    }

    /// One argument of a command or a script: a group, one character, or
    /// one command with its own arguments (`x^\frac12`).
    fn argument(&mut self) -> Node {
        self.skip_blanks();
        match self.peek() {
            Some(Token::Open) => {
                self.at += 1;
                Node::Seq(self.group())
            }
            Some(Token::Char(c)) => {
                self.at += 1;
                self.character(c, true).unwrap_or_else(empty)
            }
            Some(Token::Command(_)) if self.depth < DEEPEST => {
                self.depth += 1;
                let node = self.atom(Until::End);
                self.depth -= 1;
                node.unwrap_or_else(empty)
            }
            Some(Token::Command(_)) => {
                self.at += 1;
                empty()
            }
            _ => empty(),
        }
    }

    /// The character `c`, read; with `one`, a digit is all of a number
    /// (as `x^23` raises only the 2).
    fn character(&mut self, c: char, one: bool) -> Option<Node> {
        let node = match c {
            '0'..='9' => {
                let mut number = c.to_string();
                if !one {
                    loop {
                        match (self.peek(), self.tokens.get(self.at + 1)) {
                            (Some(Token::Char(d)), _) if d.is_ascii_digit() => number.push(d),
                            (Some(Token::Char('.')), Some(Token::Char(d)))
                                if d.is_ascii_digit() =>
                            {
                                number.push('.');
                            }
                            _ => break,
                        }
                        self.at += 1;
                    }
                }
                Node::Atom(number)
            }
            c if c.is_alphabetic() => Node::Atom(c.to_string()),
            '+' | '-' | '=' | '<' | '>' | '(' | ')' | '[' | ']' | '|' | ',' | ';' | '.' | ':'
            | '!' | '?' | '*' => Node::Op(c.to_string()),
            '/' => Node::Op("\\/".to_owned()),
            '~' => Node::Space("space.nobreak"),
            '"' | '#' | '$' | '@' | '`' | '\\' | '^' | '_' | '&' | '\'' | '√' | '∛' | '∜' => {
                Node::Atom(format!("\\{c}"))
            }
            c if c.is_control() => return None,
            // A bracket of Typst's own pairs with others: as a sign, it
            // is only the sign.
            c if typst::utils::default_math_class(c).is_some_and(|class| {
                matches!(
                    format!("{class:?}").as_str(),
                    "Opening" | "Closing" | "Fence"
                )
            }) =>
            {
                Node::Atom(format!("\\{c}"))
            }
            c => Node::Atom(c.to_string()),
        };
        Some(node)
    }

    #[allow(clippy::too_many_lines, reason = "one arm per kind of command")]
    fn command(&mut self, name: &'a str, until: Until) -> Option<Node> {
        let node = match name {
            "" | "(" | ")" | "[" | "]" | "!" => return None,
            "\\" | "cr" | "newline" => {
                self.optional();
                Node::Break
            }
            "," => Node::Space("thin"),
            ":" | ">" | ";" => Node::Space("med"),
            " " => Node::Space("space"),
            "{" => Node::Atom("brace.l".to_owned()),
            "}" => Node::Atom("brace.r".to_owned()),
            "|" => Node::Atom("bar.v.double".to_owned()),
            "%" => Node::Atom("%".to_owned()),
            "#" | "$" | "&" | "_" | "/" | "\"" => Node::Atom(format!("\\{name}")),
            "left" => self.left(),
            "right" => {
                self.delimiter();
                return None;
            }
            "middle" => {
                let delimiter = self.delimiter();
                call(
                    "mid",
                    vec![Node::Atom(delimiter_name(delimiter).to_owned())],
                )
            }
            "begin" => self.environment(),
            "end" => {
                self.name();
                return None;
            }
            "not" => self.negated(),
            "pmod" => {
                let argument = self.argument();
                Node::Seq(vec![
                    Node::Op("(".to_owned()),
                    Node::Atom("mod".to_owned()),
                    argument,
                    Node::Op(")".to_owned()),
                ])
            }
            "pod" => {
                let argument = self.argument();
                Node::Seq(vec![
                    Node::Op("(".to_owned()),
                    argument,
                    Node::Op(")".to_owned()),
                ])
            }
            _ if let Some(typst) = symbol(name) => {
                if typst.starts_with(char::is_alphabetic) {
                    Node::Atom(typst.to_owned())
                } else {
                    Node::Op(typst.to_owned())
                }
            }
            _ if let Some(shape) = command(name) => return self.shaped(shape, until),
            _ if table::SILENT.contains(&name) => return None,
            _ if name.starts_with(|c: char| c.is_ascii_alphabetic()) => {
                Converted::keep(&mut self.kept, format!("\\{name}"));
                Node::Kept(name.to_owned())
            }
            // An escaped sign nobody gave a meaning: the sign.
            _ => self
                .character(name.chars().next().unwrap_or(' '), true)
                .unwrap_or_else(empty),
        };
        Some(node)
    }

    /// What a command with arguments of `shape` makes of them.
    #[allow(clippy::too_many_lines, reason = "one arm per shape")]
    fn shaped(&mut self, shape: Shape, until: Until) -> Option<Node> {
        let node = match shape {
            Shape::Frac => {
                let above = self.argument();
                let below = self.argument();
                Node::Frac(Box::new(above), Box::new(below))
            }
            Shape::Sqrt => {
                self.skip_blanks();
                if self.peek() == Some(Token::Char('[')) {
                    self.at += 1;
                    let degree = Node::Seq(self.sequence(Until::Bracket));
                    if self.peek() == Some(Token::Char(']')) {
                        self.at += 1;
                    }
                    let radicand = self.argument();
                    call("root", vec![degree, radicand])
                } else {
                    call("sqrt", vec![self.argument()])
                }
            }
            // `\mathbb{R}` is Typst's `RR`.
            Shape::Call("bb", 1) => {
                let argument = self.argument();
                match capital(&argument) {
                    Some(letter) => Node::Atom(format!("{letter}{letter}")),
                    None => call("bb", vec![argument]),
                }
            }
            Shape::Call(name, count) => {
                let args = (0..count).map(|_| self.argument()).collect();
                call(name, args)
            }
            Shape::Text => {
                let parts = self.text_argument();
                Node::Seq(parts)
            }
            Shape::Operator => {
                let limits = self.peek() == Some(Token::Char('*'));
                if limits {
                    self.at += 1;
                }
                let name = self.raw_argument();
                Node::Call {
                    name: "op".to_owned(),
                    args: vec![Node::Text(name)],
                    named: if limits {
                        vec!["limits: #true".to_owned()]
                    } else {
                        Vec::new()
                    },
                }
            }
            Shape::Over(above) => {
                let script = self.argument();
                let base = self.argument();
                let base = Box::new(call("limits", vec![base]));
                let script = Some(Box::new(script));
                let (sub, sup) = if above {
                    (None, script)
                } else {
                    (script, None)
                };
                Node::Attach {
                    base,
                    primes: 0,
                    sub,
                    sup,
                }
            }
            Shape::Brace(name) => {
                let body = self.argument();
                self.skip_blanks();
                if matches!(self.peek(), Some(Token::Sup | Token::Sub)) {
                    self.at += 1;
                    let note = self.argument();
                    call(name, vec![body, note])
                } else {
                    call(name, vec![body])
                }
            }
            Shape::Colour(argument) => {
                let name = self.raw_argument();
                let body = if argument {
                    self.argument()
                } else {
                    Node::Seq(self.sequence(until))
                };
                if let Some(colour) = table::colour(name.trim()) {
                    Node::Call {
                        name: "text".to_owned(),
                        args: vec![body],
                        named: vec![format!("fill: #{colour}")],
                    }
                } else {
                    Converted::keep(&mut self.kept, format!("the colour {}", name.trim()));
                    body
                }
            }
            Shape::Dropped(count) => {
                self.skip_blanks();
                if self.peek() == Some(Token::Char('*')) {
                    self.at += 1;
                }
                for _ in 0..count {
                    self.argument();
                }
                return None;
            }
            Shape::Phantom => call("#hide", vec![self.argument()]),
            Shape::Boxed => Node::Call {
                name: "#box".to_owned(),
                args: vec![self.argument()],
                named: vec!["stroke: 0.5pt".to_owned(), "inset: 3pt".to_owned()],
            },
            Shape::Arrow(arrow) => {
                let below = self.optional();
                let above = self.argument();
                Node::Attach {
                    base: Box::new(call("limits", vec![Node::Atom(arrow.to_owned())])),
                    primes: 0,
                    sub: below.map(Box::new),
                    sup: Some(Box::new(above)),
                }
            }
        };
        Some(node)
    }

    /// An argument in square brackets, if one stands next.
    fn optional(&mut self) -> Option<Node> {
        let save = self.at;
        self.skip_blanks();
        if self.peek() != Some(Token::Char('[')) {
            self.at = save;
            return None;
        }
        self.at += 1;
        let inside = Node::Seq(self.sequence(Until::Bracket));
        if self.peek() == Some(Token::Char(']')) {
            self.at += 1;
        }
        Some(inside)
    }

    /// The argument of a command as the text it is written with.
    fn raw_argument(&mut self) -> String {
        self.skip_blanks();
        match self.next() {
            Some(Token::Open) => self.raw_group(),
            Some(token) => written(token),
            None => String::new(),
        }
    }

    /// `\text{…}`: the text, and what is between dollar signs in it as
    /// mathematics.
    fn text_argument(&mut self) -> Vec<Node> {
        self.skip_blanks();
        let tokens: Vec<Token<'a>> = match self.next() {
            Some(Token::Open) => {
                let start = self.at;
                let mut depth = 1;
                while let Some(token) = self.next() {
                    match token {
                        Token::Open => depth += 1,
                        Token::Close => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let end = self.at.saturating_sub(1).max(start);
                self.tokens[start..end.min(self.tokens.len())].to_vec()
            }
            Some(token) => vec![token],
            None => Vec::new(),
        };
        let mut parts = Vec::new();
        let mut text = String::new();
        let pieces = tokens.split(|token| *token == Token::Char('$'));
        let mut math = false;
        for piece in pieces {
            if math {
                let mut inner = Parser::new(piece.to_vec());
                inner.depth = self.depth + 1;
                if !text.is_empty() {
                    parts.push(Node::Text(std::mem::take(&mut text)));
                }
                parts.push(Node::Seq(inner.sequence(Until::End)));
                for what in inner.kept {
                    Converted::keep(&mut self.kept, what);
                }
            } else {
                for token in piece {
                    text.push_str(&in_text(*token));
                }
            }
            math = !math;
        }
        if !text.is_empty() || parts.is_empty() {
            parts.push(Node::Text(text));
        }
        parts
    }

    /// The name of an environment: `{pmatrix}`.
    fn name(&mut self) -> String {
        self.raw_argument().trim().to_owned()
    }

    /// One of the delimiters `\left` and `\right` take, as Typst writes it.
    fn delimiter(&mut self) -> &'static str {
        self.skip_blanks();
        let written = match self.next() {
            Some(Token::Char(c)) => c.to_string(),
            Some(Token::Command(name)) => format!("\\{name}"),
            _ => return "",
        };
        table::delimiter(&written).unwrap_or("")
    }

    /// `\left(` … `\right)`, the `\left` taken.
    fn left(&mut self) -> Node {
        let open = self.delimiter();
        let body = self.sequence(Until::Right);
        let close = if self.peek() == Some(Token::Command("right")) {
            self.at += 1;
            self.delimiter()
        } else {
            ""
        };
        Node::Lr(open, body, close)
    }

    /// `\begin{…}` … `\end{…}`, the `\begin` taken.
    fn environment(&mut self) -> Node {
        let name = self.name();
        let layout = table::environment(&name).unwrap_or_else(|| {
            Converted::keep(&mut self.kept, format!("\\begin{{{name}}}"));
            table::Layout::Lines
        });
        // The columns of an array, how many pairs `alignat` has.
        if matches!(name.as_str(), "array" | "alignat" | "alignat*") {
            self.raw_argument();
        }
        let mut rows = Vec::new();
        let mut row = Vec::new();
        loop {
            let cell = self.sequence(Until::Cell);
            row.push(cell);
            match self.next() {
                Some(Token::Amp) => {}
                Some(Token::Command("\\" | "cr")) => {
                    self.optional();
                    rows.push(std::mem::take(&mut row));
                }
                Some(Token::Command("end")) => {
                    self.name();
                    break;
                }
                // A brace closing what the environment is in, or the end.
                other => {
                    if other == Some(Token::Close) {
                        self.at -= 1;
                    }
                    break;
                }
            }
        }
        rows.push(row);
        // A line break behind the last row makes no row.
        if rows.len() > 1 && rows.last().is_some_and(|row| row.iter().all(Vec::is_empty)) {
            rows.pop();
        }
        Node::Grid(layout, rows)
    }

    /// `\not` and what it is said of.
    fn negated(&mut self) -> Node {
        self.skip_blanks();
        let Some(node) = self.atom(Until::End) else {
            return Node::Op("\\/".to_owned());
        };
        let negated = match &node {
            Node::Op(op) => match op.as_str() {
                "=" => Some("!="),
                "<" => Some("lt.not"),
                ">" => Some("gt.not"),
                "<=" => Some("lt.eq.not"),
                ">=" => Some("gt.eq.not"),
                _ => None,
            },
            Node::Atom(name) => match name.as_str() {
                "in" => Some("in.not"),
                "subset" => Some("subset.not"),
                "supset" => Some("supset.not"),
                "subset.eq" => Some("subset.eq.not"),
                "supset.eq" => Some("supset.eq.not"),
                "equiv" => Some("equiv.not"),
                "approx" => Some("approx.not"),
                "tilde.op" => Some("tilde.not"),
                "tilde.eq" => Some("tilde.eq.not"),
                "tilde.equiv" => Some("tilde.equiv.not"),
                "prec" => Some("prec.not"),
                "succ" => Some("succ.not"),
                "parallel" => Some("parallel.not"),
                "divides" => Some("divides.not"),
                "exists" => Some("exists.not"),
                _ => None,
            },
            _ => None,
        };
        match negated {
            Some("!=") => Node::Op("!=".to_owned()),
            Some(name) => Node::Atom(name.to_owned()),
            None => call("cancel", vec![node]),
        }
    }
}

fn call(name: &str, args: Vec<Node>) -> Node {
    Node::Call {
        name: name.to_owned(),
        args,
        named: Vec::new(),
    }
}

/// The capital letter `node` is, alone.
fn capital(node: &Node) -> Option<char> {
    match node {
        Node::Atom(letter) => {
            let mut chars = letter.chars();
            let c = chars.next()?;
            (chars.next().is_none() && c.is_ascii_uppercase()).then_some(c)
        }
        Node::Seq(items) if items.len() == 1 => capital(&items[0]),
        _ => None,
    }
}

/// A token as LaTeX writes it.
fn written(token: Token) -> String {
    match token {
        Token::Command(name) => format!("\\{name}"),
        Token::Open => "{".to_owned(),
        Token::Close => "}".to_owned(),
        Token::Sup => "^".to_owned(),
        Token::Sub => "_".to_owned(),
        Token::Amp => "&".to_owned(),
        Token::Primes(count) => "'".repeat(count),
        Token::Blank => " ".to_owned(),
        Token::Char(c) => c.to_string(),
    }
}

/// A token of `\text{…}` as the text it shows.
fn in_text(token: Token) -> String {
    match token {
        Token::Command(" " | "\\" | "," | ";" | "quad" | "qquad") | Token::Char('~') => {
            " ".to_owned()
        }
        Token::Command(sign @ ("{" | "}" | "%" | "&" | "#" | "_" | "$")) => sign.to_owned(),
        Token::Command("ldots" | "dots") => "…".to_owned(),
        // What sets text off is not followed: its text is.
        Token::Open
        | Token::Close
        | Token::Command(
            "textbf" | "textit" | "emph" | "textrm" | "textsf" | "texttt" | "text" | "mbox",
        ) => String::new(),
        token => written(token),
    }
}

// ---- Writing Typst ---------------------------------------------------

/// `nodes` as the Typst of one sequence; `args` where they are in the
/// arguments of a call, whose commas and semicolons part arguments.
fn print_all(nodes: &[Node], args: bool) -> String {
    let mut pieces = Vec::new();
    for node in nodes {
        pieces_of(node, args, &mut pieces);
    }
    balance(&mut pieces);
    join(&pieces)
}

/// Brackets that pair with none are written as signs: Typst pairs any
/// opening bracket with the next closing one, the one that closes a call
/// too.
fn balance(pieces: &mut [String]) {
    let mut open = Vec::new();
    let mut lone = Vec::new();
    for (index, piece) in pieces.iter().enumerate() {
        match piece.as_str() {
            "(" | "[" => open.push(index),
            ")" | "]" if open.pop().is_none() => lone.push(index),
            _ => {}
        }
    }
    for index in open.into_iter().chain(lone) {
        pieces[index].insert(0, '\\');
    }
}

fn pieces_of(node: &Node, args: bool, pieces: &mut Vec<String>) {
    match node {
        Node::Seq(items) => {
            for item in items {
                pieces_of(item, args, pieces);
            }
        }
        node => pieces.push(print(node, args)),
    }
}

/// Pieces of Typst one after the other, with a blank between two where
/// they would otherwise be read as one (`x` and `2`, `alpha` and `x`,
/// `<` and `=`), and where it reads better.
fn join(pieces: &[String]) -> String {
    let mut out = String::new();
    let mut last: &str = "";
    for piece in pieces.iter().filter(|piece| !piece.is_empty()) {
        // A number and the letter it multiplies: `2n`.
        let number = last.starts_with(|c: char| c.is_ascii_digit())
            && last.chars().all(|c| c.is_ascii_digit() || c == '.');
        let factor = number && piece.chars().count() == 1 && piece.starts_with(char::is_alphabetic);
        if let (Some(a), Some(b)) = (out.chars().next_back(), piece.chars().next())
            && !factor
            && blank_between(a, b, last, piece)
        {
            out.push(' ');
        }
        out.push_str(piece);
        last = piece;
    }
    out
}

fn blank_between(a: char, b: char, last: &str, piece: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let shorthand = |c: char| "-:!.<>=|~*[]+/".contains(c);
    if a == '\n' {
        return false;
    }
    if (word(a) || a == '.') && word(b) {
        return true;
    }
    if shorthand(a) && shorthand(b) {
        return true;
    }
    if matches!(piece, "," | ";" | ")" | "]" | "!" | ".") || matches!(a, '(' | '[') {
        return false;
    }
    // A letter and its argument: `f(x)`.
    let letter = last.chars().count() == 1 && a.is_alphabetic();
    !(b == '(' && (letter || a == '\''))
}

fn print(node: &Node, args: bool) -> String {
    match node {
        Node::Atom(text) => text.clone(),
        Node::Op(op) if args && (op == "," || op == ";") => format!("\\{op}"),
        Node::Op(op) => op.clone(),
        Node::Seq(items) => print_all(items, args),
        Node::Attach {
            base,
            primes,
            sub,
            sup,
        } => {
            let mut out = attach_base(base, args);
            out.push_str(&"'".repeat(*primes));
            if let Some(sub) = sub {
                out.push('_');
                out.push_str(&script(sub));
            }
            if let Some(sup) = sup {
                out.push('^');
                out.push_str(&script(sup));
            }
            out
        }
        Node::Frac(above, below) => format!("{}/{}", part(above), part(below)),
        // Code, where mathematics has no such function: `#box($x$)`.
        Node::Call { name, args, named } if name.starts_with('#') => {
            let mut all: Vec<String> = args
                .iter()
                .map(|arg| format!("${}$", argument(arg)))
                .collect();
            all.extend(named.iter().cloned());
            format!("{name}({})", all.join(", "))
        }
        Node::Call { name, args, named } => {
            let mut all: Vec<String> = args.iter().map(argument).collect();
            all.extend(named.iter().cloned());
            format!("{name}({})", all.join(", "))
        }
        Node::Text(text) => quoted(text),
        Node::Lr(open, body, close) => lr(open, body, close, args),
        Node::Grid(layout, rows) => grid(*layout, rows, args),
        Node::Break => "\\\n".to_owned(),
        Node::Align => "&".to_owned(),
        Node::Space(space) => (*space).to_owned(),
        Node::Kept(name) => quoted(&format!("\\{name}")),
    }
}

/// One node by itself, its brackets paired.
fn alone(node: &Node, args: bool) -> String {
    print_all(std::slice::from_ref(node), args)
}

/// An argument of a call: never nothing, which would be none.
fn argument(node: &Node) -> String {
    let printed = alone(node, true);
    if printed.trim().is_empty() {
        "\"\"".to_owned()
    } else {
        printed
    }
}

/// The one thing `node` is, if it is a group of one.
fn single(node: &Node) -> &Node {
    match node {
        Node::Seq(items) if items.len() == 1 => single(&items[0]),
        node => node,
    }
}

fn is_nothing(node: &Node) -> bool {
    matches!(single(node), Node::Seq(items) if items.is_empty())
}

/// What a script is raised or lowered on.
fn attach_base(node: &Node, args: bool) -> String {
    match single(node) {
        _ if is_nothing(node) => "\"\"".to_owned(),
        node @ (Node::Atom(_)
        | Node::Op(_)
        | Node::Call { .. }
        | Node::Text(_)
        | Node::Kept(_)
        | Node::Lr(..)) => alone(node, args),
        // What braces group is raised as one, as nothing would show it.
        node => format!("lr({})", alone(node, true)),
    }
}

/// What is raised or lowered.
fn script(node: &Node) -> String {
    match single(node) {
        _ if is_nothing(node) => "\"\"".to_owned(),
        node @ (Node::Atom(_) | Node::Call { .. } | Node::Text(_) | Node::Kept(_)) => {
            alone(node, false)
        }
        node => format!("({})", alone(node, false)),
    }
}

/// What is above or below the line of a fraction.
fn part(node: &Node) -> String {
    match single(node) {
        node @ (Node::Atom(_) | Node::Call { .. } | Node::Text(_) | Node::Kept(_)) => {
            alone(node, false)
        }
        node @ Node::Attach { base, .. }
            if matches!(
                single(base),
                Node::Atom(_) | Node::Call { .. } | Node::Text(_)
            ) =>
        {
            alone(node, false)
        }
        node => format!("({})", alone(node, false)),
    }
}

fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// How a delimiter is named in Typst where it stands alone.
fn delimiter_name(delimiter: &str) -> &str {
    match delimiter {
        "(" => "paren.l",
        ")" => "paren.r",
        "[" => "bracket.l",
        "]" => "bracket.r",
        "{" => "brace.l",
        "}" => "brace.r",
        "|" => "bar.v",
        "‖" => "bar.v.double",
        "⟨" => "chevron.l",
        "⟩" => "chevron.r",
        "⌊" => "floor.l",
        "⌋" => "floor.r",
        "⌈" => "ceil.l",
        "⌉" => "ceil.r",
        "\\/" => "slash",
        other => other,
    }
}

fn lr(open: &str, body: &[Node], close: &str, args: bool) -> String {
    let inside = print_all(body, args);
    let opening = "([{⟨⌊⌈".contains(open) && !open.is_empty();
    let closing = ")]}⟩⌋⌉".contains(close) && !close.is_empty();
    // Typst makes brackets that pair as large as what is between them.
    if opening && closing {
        return format!("{open}{inside}{close}");
    }
    match (open, close) {
        ("|", "|") => return format!("abs({})", nonempty(print_all(body, true))),
        ("‖", "‖") => return format!("norm({})", nonempty(print_all(body, true))),
        _ => {}
    }
    let inside = print_all(body, true);
    let parts: Vec<&str> = [delimiter_name(open), inside.as_str(), delimiter_name(close)]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();
    format!("lr({})", nonempty(parts.join(" ")))
}

fn nonempty(text: String) -> String {
    if text.trim().is_empty() {
        "\"\"".to_owned()
    } else {
        text
    }
}

fn grid(layout: table::Layout, rows: &[Vec<Vec<Node>>], args: bool) -> String {
    match layout {
        table::Layout::Matrix(delimiter) => {
            let rows: Vec<String> = rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| print_all(cell, true))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .collect();
            let cells = rows.join("; ");
            match delimiter {
                Some(delimiter) if cells.trim().is_empty() => format!("mat(delim: {delimiter})"),
                Some(delimiter) => format!("mat(delim: {delimiter}, {cells})"),
                None => format!("mat({cells})"),
            }
        }
        table::Layout::Cases => {
            let rows: Vec<String> = rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| print_all(cell, true))
                        .collect::<Vec<_>>()
                        .join(" & ")
                })
                .collect();
            format!("cases({})", rows.join(", "))
        }
        table::Layout::Lines => rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| print_all(cell, args))
                    .collect::<Vec<_>>()
                    .join(" & ")
                    .trim()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join(" \\\n"),
    }
}

#[cfg(test)]
mod tests;
