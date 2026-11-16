//! Typst's mathematics as LaTeX's, for copying an equation into another
//! program: what the converter reads, written back. What has no LaTeX
//! here (code, a function nobody mapped) is named in
//! [`Converted::kept`](super::Converted).

use std::collections::HashMap;
use std::sync::OnceLock;

use typst::syntax::{SyntaxKind, SyntaxNode};

use super::Converted;
use super::table::{self, COMMANDS, Shape};

/// The mathematics of an equation, what is between its dollar signs, as
/// LaTeX. Lines and places to align at make an `aligned` environment.
#[must_use]
pub fn to_latex(typst: &str) -> Converted {
    let root = typst::syntax::parse_math(typst);
    let mut back = Back { kept: Vec::new() };
    let mut text = back.node(&root).trim().to_owned();
    let lines = root.children().any(|child| {
        matches!(
            child.kind(),
            SyntaxKind::Linebreak | SyntaxKind::MathAlignPoint
        )
    });
    if lines {
        text = format!("\\begin{{aligned}}\n{text}\n\\end{{aligned}}");
    }
    Converted {
        text,
        kept: back.kept,
    }
}

struct Back {
    kept: Vec<String>,
}

/// Puts `piece` behind `out`, with a blank where a command's name would
/// otherwise run on into a letter.
fn push(out: &mut String, piece: &str) {
    let command = {
        let name = out.trim_end_matches(|c: char| c.is_ascii_alphabetic());
        name.len() < out.len() && name.ends_with('\\')
    };
    if command && piece.starts_with(|c: char| c.is_ascii_alphabetic()) {
        out.push(' ');
    }
    out.push_str(piece);
}

/// The characters of Typst's symbols, and the LaTeX names of them.
fn characters() -> &'static HashMap<char, &'static str> {
    static MAP: OnceLock<HashMap<char, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        use typst::foundations::Value;
        use typst::{Library, LibraryExt};
        let library = Library::default();
        let mut map = HashMap::new();
        for (latex, typst) in table::SYMBOLS {
            let mut parts = typst.split('.');
            let Some(first) = parts.next() else {
                continue;
            };
            let Some(Value::Symbol(symbol)) = library
                .math
                .scope()
                .get(first)
                .map(typst::foundations::Binding::read)
            else {
                continue;
            };
            let mut symbol = Some(symbol.clone());
            for modifier in parts {
                symbol = symbol.and_then(|symbol| symbol.modified((), modifier).ok());
            }
            let Some(symbol) = symbol else {
                continue;
            };
            let mut chars = symbol.get().chars();
            if let (Some(c), None) = (chars.next(), chars.next())
                && !c.is_ascii()
            {
                map.entry(c).or_insert(*latex);
            }
        }
        map
    })
}

impl Back {
    fn keep(&mut self, what: impl Into<String>) {
        Converted::keep(&mut self.kept, what.into());
    }

    fn children(&mut self, node: &SyntaxNode) -> String {
        let mut out = String::new();
        for child in node.children() {
            let piece = self.node(child);
            push(&mut out, &piece);
        }
        out
    }

    #[allow(clippy::too_many_lines, reason = "one arm per kind of node")]
    fn node(&mut self, node: &SyntaxNode) -> String {
        let text = node.leaf_text().as_str();
        match node.kind() {
            SyntaxKind::Math => self.children(node),
            SyntaxKind::Space => {
                if text.contains('\n') {
                    "\n".to_owned()
                } else {
                    " ".to_owned()
                }
            }
            SyntaxKind::MathText => text.chars().map(character).collect(),
            SyntaxKind::MathIdent => self.name(text),
            SyntaxKind::MathFieldAccess => self.name(&node.full_text()),
            SyntaxKind::MathShorthand => self.shorthand(text),
            SyntaxKind::Escape => escape(text),
            SyntaxKind::Str => string(text),
            SyntaxKind::MathPrimes => text.to_owned(),
            SyntaxKind::MathAlignPoint => "&".to_owned(),
            SyntaxKind::Linebreak => "\\\\".to_owned(),
            SyntaxKind::MathAttach => self.attach(node),
            SyntaxKind::MathFrac => {
                let mut parts = node
                    .children()
                    .filter(|child| !matches!(child.kind(), SyntaxKind::Slash | SyntaxKind::Space));
                let above = parts
                    .next()
                    .map(|part| self.braced(part))
                    .unwrap_or_default();
                let below = parts
                    .next()
                    .map(|part| self.braced(part))
                    .unwrap_or_default();
                format!("\\frac{{{above}}}{{{below}}}")
            }
            SyntaxKind::MathRoot => {
                let degree = match text
                    .chars()
                    .next()
                    .or_else(|| node.full_text().chars().next())
                {
                    Some('∛') => "[3]",
                    Some('∜') => "[4]",
                    _ => "",
                };
                let radicand = node
                    .children()
                    .find(|child| child.kind() != SyntaxKind::Root)
                    .map(|part| self.braced(part))
                    .unwrap_or_default();
                format!("\\sqrt{degree}{{{radicand}}}")
            }
            SyntaxKind::MathDelimited => self.delimited(node),
            SyntaxKind::MathCall => self.call(node),
            SyntaxKind::LeftParen | SyntaxKind::RightParen => text.to_owned(),
            SyntaxKind::LeftBrace | SyntaxKind::RightBrace => delimiter(text, false),
            SyntaxKind::Dot
            | SyntaxKind::Comma
            | SyntaxKind::Semicolon
            | SyntaxKind::Bang
            | SyntaxKind::Colon => text.to_owned(),
            SyntaxKind::Hash => String::new(),
            SyntaxKind::FuncCall
            | SyntaxKind::FieldAccess
            | SyntaxKind::Ident
            | SyntaxKind::Code
            | SyntaxKind::CodeBlock
            | SyntaxKind::ContentBlock => {
                self.keep(format!("#{}", node.full_text().trim_start_matches('#')));
                String::new()
            }
            _ if node.children().len() > 0 => self.children(node),
            _ => text.to_owned(),
        }
    }

    /// A part of a fraction or a script, without the brackets Typst
    /// takes away there.
    fn braced(&mut self, node: &SyntaxNode) -> String {
        // Typst has made the brackets there no brackets.
        if node.kind() == SyntaxKind::Math {
            let children: Vec<&SyntaxNode> = node.children().collect();
            if children.len() >= 2
                && children[0].kind() == SyntaxKind::LeftParen
                && children[children.len() - 1].kind() == SyntaxKind::RightParen
            {
                return children[1..children.len() - 1]
                    .iter()
                    .map(|child| self.node(child))
                    .fold(String::new(), |mut out, piece| {
                        push(&mut out, &piece);
                        out
                    })
                    .trim()
                    .to_owned();
            }
        }
        self.node(node).trim().to_owned()
    }

    fn name(&mut self, name: &str) -> String {
        if let Some(latex) = table::latex_of(name) {
            return format!("\\{latex}");
        }
        match name {
            "oo" => "\\infty".to_owned(),
            "dif" => "\\mathrm{d}".to_owned(),
            "Dif" => "\\mathrm{D}".to_owned(),
            "thin" => "\\,".to_owned(),
            "med" => "\\:".to_owned(),
            "thick" => "\\;".to_owned(),
            "wide" => "\\qquad".to_owned(),
            "dots" => "\\dots".to_owned(),
            "space" => "\\ ".to_owned(),
            "space.nobreak" => "~".to_owned(),
            "paren.l" => "(".to_owned(),
            "paren.r" => ")".to_owned(),
            "csch" | "sech" | "sinc" | "ctg" | "tg" | "id" | "im" | "tr" => {
                format!("\\operatorname{{{name}}}")
            }
            double
                if double.len() == 2
                    && double.chars().all(|c| c.is_ascii_uppercase())
                    && double[..1] == double[1..] =>
            {
                format!("\\mathbb{{{}}}", &double[..1])
            }
            _ => {
                self.keep(name);
                format!("\\mathrm{{{name}}}")
            }
        }
    }

    fn shorthand(&mut self, text: &str) -> String {
        if let Some(latex) = table::latex_of(text) {
            return format!("\\{latex}");
        }
        let latex = match text {
            "-" => "-",
            "*" => "\\ast",
            "~" => "\\sim",
            "..." => "\\dots",
            "||" => "\\|",
            "[|" => "\\llbracket",
            "|]" => "\\rrbracket",
            "=:" => "\\eqqcolon",
            "->>" => "\\twoheadrightarrow",
            "<<-" => "\\twoheadleftarrow",
            "~>" => "\\rightsquigarrow",
            "<~" => "\\leftsquigarrow",
            "<<<" => "\\lll",
            ">>>" => "\\ggg",
            "|=>" => "\\Mapsto",
            _ => {
                self.keep(text);
                return text.to_owned();
            }
        };
        latex.to_owned()
    }

    fn attach(&mut self, node: &SyntaxNode) -> String {
        let mut children = node.children();
        let Some(base) = children.next() else {
            return String::new();
        };
        let (mut sub, mut sup, mut primes) = (None, None, String::new());
        let mut mark = None;
        for child in children {
            match child.kind() {
                SyntaxKind::Underscore | SyntaxKind::Hat => mark = Some(child.kind()),
                SyntaxKind::MathPrimes => primes = child.leaf_text().to_string(),
                SyntaxKind::Space => {}
                _ => match mark.take() {
                    Some(SyntaxKind::Underscore) => sub = Some(self.braced(child)),
                    Some(_) => sup = Some(self.braced(child)),
                    None => {}
                },
            }
        }
        // `limits(x)^a` is something set over another.
        if let Some(inner) = limits(base) {
            let inner = self.braced(inner);
            return match (sub, sup) {
                (None, Some(sup)) => format!("\\overset{{{sup}}}{{{inner}}}"),
                (Some(sub), None) => format!("\\underset{{{sub}}}{{{inner}}}"),
                (sub, sup) => {
                    let mut out = format!("\\mathop{{{inner}}}\\limits");
                    if let Some(sub) = sub {
                        out.push_str(&format!("_{{{sub}}}"));
                    }
                    if let Some(sup) = sup {
                        out.push_str(&format!("^{{{sup}}}"));
                    }
                    out
                }
            };
        }
        let mut out = match base.kind() {
            SyntaxKind::Str if base.leaf_text() == "\"\"" => "{}".to_owned(),
            SyntaxKind::Math => format!("{{{}}}", self.node(base).trim()),
            _ => self.node(base),
        };
        out.push_str(&primes);
        if let Some(sub) = sub {
            out.push_str(&format!("_{{{sub}}}"));
        }
        if let Some(sup) = sup {
            out.push_str(&format!("^{{{sup}}}"));
        }
        out
    }

    fn delimited(&mut self, node: &SyntaxNode) -> String {
        let children: Vec<&SyntaxNode> = node.children().collect();
        let (Some(first), Some(last)) = (children.first(), children.last()) else {
            return String::new();
        };
        let mut inside = String::new();
        for child in &children[1..children.len().saturating_sub(1).max(1)] {
            let piece = self.node(child);
            push(&mut inside, &piece);
        }
        let tall = node.children().any(is_tall);
        let open = delimiter(first.leaf_text(), tall);
        let close = if children.len() > 1 {
            delimiter(last.leaf_text(), tall)
        } else {
            String::new()
        };
        if tall {
            format!("\\left{open} {} \\right{close}", inside.trim())
        } else {
            format!("{open}{inside}{close}")
        }
    }

    /// The arguments of a call: rows of cells (parted by `;` and `,`),
    /// and what is named.
    fn arguments(&mut self, node: &SyntaxNode) -> (Vec<Vec<String>>, Vec<(String, String)>) {
        let mut rows = vec![Vec::new()];
        let mut named = Vec::new();
        let Some(args) = node
            .children()
            .find(|child| child.kind() == SyntaxKind::MathArgs)
        else {
            return (rows, named);
        };
        let mut cell = String::new();
        let mut filled = false;
        let mut after_named = false;
        for child in args.children() {
            match child.kind() {
                SyntaxKind::LeftParen | SyntaxKind::RightParen | SyntaxKind::Space => {}
                // The comma behind what is named parts no cells.
                SyntaxKind::Comma if after_named => after_named = false,
                SyntaxKind::Comma | SyntaxKind::Semicolon => {
                    if let Some(row) = rows.last_mut() {
                        row.push(std::mem::take(&mut cell).trim().to_owned());
                    }
                    filled = false;
                    if child.kind() == SyntaxKind::Semicolon {
                        rows.push(Vec::new());
                    }
                }
                SyntaxKind::Named => {
                    let mut parts = child.children();
                    let key = parts.next().map(|key| key.leaf_text().to_string());
                    let value: String = parts
                        .filter(|part| {
                            !matches!(part.kind(), SyntaxKind::Colon | SyntaxKind::Space)
                        })
                        .map(|part| part.full_text().to_string())
                        .collect();
                    named.push((key.unwrap_or_default(), value.trim().to_owned()));
                    filled = false;
                    after_named = true;
                }
                _ => {
                    let piece = self.node(child);
                    push(&mut cell, &piece);
                    filled = true;
                }
            }
        }
        if filled || !cell.trim().is_empty() {
            if let Some(row) = rows.last_mut() {
                row.push(cell.trim().to_owned());
            }
        }
        rows.retain(|row| !row.is_empty());
        (rows, named)
    }

    #[allow(clippy::too_many_lines, reason = "one arm per function")]
    fn call(&mut self, node: &SyntaxNode) -> String {
        let name = node
            .children()
            .next()
            .map(|callee| callee.full_text().to_string())
            .unwrap_or_default();
        let (rows, named) = self.arguments(node);
        let args: Vec<String> = rows.iter().flatten().cloned().collect();
        let arg = |index: usize| args.get(index).cloned().unwrap_or_default();
        let named_value = |key: &str| {
            named
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        };
        match name.as_str() {
            "frac" => format!("\\frac{{{}}}{{{}}}", arg(0), arg(1)),
            "sqrt" => format!("\\sqrt{{{}}}", arg(0)),
            "root" => format!("\\sqrt[{}]{{{}}}", arg(0), arg(1)),
            "abs" => format!("\\left| {} \\right|", arg(0)),
            "norm" => format!("\\left\\| {} \\right\\|", arg(0)),
            "floor" => format!("\\left\\lfloor {} \\right\\rfloor", arg(0)),
            "ceil" => format!("\\left\\lceil {} \\right\\rceil", arg(0)),
            "round" => format!("\\left\\lfloor {} \\right\\rceil", arg(0)),
            "lr" => lr(&arg(0)),
            "op" => {
                let star = if named_value("limits").is_some_and(|value| value.contains("true")) {
                    "*"
                } else {
                    ""
                };
                let inner = arg(0);
                let inner = inner
                    .strip_prefix("\\text{")
                    .and_then(|inner| inner.strip_suffix('}'))
                    .unwrap_or(&inner);
                format!("\\operatorname{star}{{{inner}}}")
            }
            "underbrace" | "overbrace" => {
                let mark = if name == "underbrace" { '_' } else { '^' };
                match args.get(1) {
                    Some(note) => format!("\\{name}{{{}}}{mark}{{{note}}}", arg(0)),
                    None => format!("\\{name}{{{}}}", arg(0)),
                }
            }
            "limits" | "scripts" if args.len() == 1 => arg(0),
            "display" => format!("{{\\displaystyle {}}}", arg(0)),
            "inline" => format!("{{\\textstyle {}}}", arg(0)),
            "hide" => format!("\\phantom{{{}}}", arg(0)),
            "text" => {
                let colour = named_value("fill")
                    .map(|fill| fill.trim_start_matches('#').to_owned())
                    .and_then(|fill| {
                        table::COLOURS
                            .iter()
                            .find(|(_, typst)| *typst == fill)
                            .map(|(latex, _)| *latex)
                    });
                match colour {
                    Some(colour) => format!("\\textcolor{{{colour}}}{{{}}}", arg(0)),
                    None => arg(0),
                }
            }
            "box" => format!("\\boxed{{{}}}", arg(0)),
            "mid" => format!("\\middle{}", arg(0)),
            "mat" | "vec" | "cases" => {
                let rows = if name == "vec" || name == "cases" {
                    args.iter().map(|cell| vec![cell.clone()]).collect()
                } else {
                    rows
                };
                let environment = match name.as_str() {
                    "cases" => "cases",
                    _ => match named_value("delim").as_deref().map(str::trim) {
                        Some("\"[\"") => "bmatrix",
                        Some("\"{\"") => "Bmatrix",
                        Some("\"|\"") => "vmatrix",
                        Some("\"||\"" | "\"‖\"") => "Vmatrix",
                        Some("#none" | "none") => "matrix",
                        _ => "pmatrix",
                    },
                };
                let body = rows
                    .iter()
                    .map(|row| row.join(" & "))
                    .collect::<Vec<_>>()
                    .join(" \\\\ ");
                format!("\\begin{{{environment}}} {body} \\end{{{environment}}}")
            }
            _ => match one_argument(&name) {
                Some(latex) if args.len() == 1 => format!("\\{latex}{{{}}}", arg(0)),
                Some(latex) if latex == "binom" => {
                    format!("\\binom{{{}}}{{{}}}", arg(0), arg(1))
                }
                _ => {
                    // A symbol called as an accent, or a function of the
                    // document's own.
                    self.keep(format!("{name}(…)"));
                    let callee = self.name(&name);
                    format!("{callee}\\left( {} \\right)", args.join(", "))
                }
            },
        }
    }
}

/// The LaTeX command of the Typst function `name`, first of the table.
fn one_argument(name: &str) -> Option<&'static str> {
    COMMANDS.iter().find_map(|(latex, shape)| match shape {
        Shape::Call(typst, _) if *typst == name => Some(*latex),
        _ => None,
    })
}

/// What `limits(…)` holds, if `node` is that.
fn limits(node: &SyntaxNode) -> Option<&SyntaxNode> {
    if node.kind() != SyntaxKind::MathCall {
        return None;
    }
    let mut children = node.children();
    if children.next()?.leaf_text() != "limits" {
        return None;
    }
    let args = children.find(|child| child.kind() == SyntaxKind::MathArgs)?;
    args.children()
        .find(|child| !matches!(child.kind(), SyntaxKind::LeftParen | SyntaxKind::RightParen))
}

/// Whether `node` is something LaTeX would want `\left` and `\right`
/// around.
fn is_tall(node: &SyntaxNode) -> bool {
    match node.kind() {
        SyntaxKind::MathFrac | SyntaxKind::MathRoot => true,
        SyntaxKind::MathCall => node.children().next().is_some_and(|callee| {
            matches!(
                callee.leaf_text().as_str(),
                "frac" | "binom" | "mat" | "vec" | "cases" | "sqrt" | "root"
            )
        }),
        _ => node.children().any(is_tall),
    }
}

/// `lr(…)`: its first and last delimiters made `\left` and `\right`.
fn lr(inside: &str) -> String {
    let opening = [
        "(", "[", "\\{", "\\langle", "|", "\\|", "\\lfloor", "\\lceil",
    ];
    let closing = [
        ")", "]", "\\}", "\\rangle", "|", "\\|", "\\rfloor", "\\rceil",
    ];
    let trimmed = inside.trim();
    let open = opening.iter().find(|open| trimmed.starts_with(**open));
    let close = closing.iter().find(|close| trimmed.ends_with(**close));
    match (open, close) {
        (Some(open), Some(close)) if trimmed.len() >= open.len() + close.len() => {
            let body = &trimmed[open.len()..trimmed.len() - close.len()];
            format!("\\left{open} {} \\right{close}", body.trim())
        }
        (Some(open), _) => format!("\\left{open} {} \\right.", trimmed[open.len()..].trim()),
        (_, Some(close)) => format!(
            "\\left. {} \\right{close}",
            trimmed[..trimmed.len() - close.len()].trim()
        ),
        _ => trimmed.to_owned(),
    }
}

/// A delimiter of Typst as LaTeX writes it; `tall` where it is one of
/// `\left` or `\right`.
fn delimiter(text: &str, tall: bool) -> String {
    match text {
        "{" => "\\{".to_owned(),
        "}" => "\\}".to_owned(),
        "⟨" => "\\langle".to_owned(),
        "⟩" => "\\rangle".to_owned(),
        "⌊" => "\\lfloor".to_owned(),
        "⌋" => "\\rfloor".to_owned(),
        "⌈" => "\\lceil".to_owned(),
        "⌉" => "\\rceil".to_owned(),
        "‖" => "\\|".to_owned(),
        "[|" if tall => "[".to_owned(),
        other => other.to_owned(),
    }
}

fn character(c: char) -> String {
    match c {
        '{' => "\\{".to_owned(),
        '}' => "\\}".to_owned(),
        '%' | '#' | '&' | '_' | '$' => format!("\\{c}"),
        c if c.is_ascii() => c.to_string(),
        c => characters()
            .get(&c)
            .map_or_else(|| c.to_string(), |latex| format!("\\{latex}")),
    }
}

fn escape(text: &str) -> String {
    match text.trim_start_matches('\\') {
        "#" => "\\#".to_owned(),
        "$" => "\\$".to_owned(),
        "_" => "\\_".to_owned(),
        "&" => "\\&".to_owned(),
        "%" => "\\%".to_owned(),
        "{" => "\\{".to_owned(),
        "}" => "\\}".to_owned(),
        "\\" => "\\backslash".to_owned(),
        other if other.starts_with("u{") => {
            let code = other.trim_start_matches("u{").trim_end_matches('}');
            u32::from_str_radix(code, 16)
                .ok()
                .and_then(char::from_u32)
                .map_or_else(String::new, character)
        }
        other => other.to_owned(),
    }
}

/// Text in an equation, `"…"`, as `\text{…}`; text that is a LaTeX name
/// kept as typed (`"\foo"`) is the name again.
fn string(written: &str) -> String {
    let inner = written
        .strip_prefix('"')
        .map_or(written, |inner| inner.strip_suffix('"').unwrap_or(inner));
    let mut text = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => text.push('\n'),
                Some('t') => text.push('\t'),
                Some(other) => text.push(other),
                None => {}
            }
        } else {
            text.push(c);
        }
    }
    let name = text.strip_prefix('\\');
    if name.is_some_and(|name| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphabetic())) {
        return text;
    }
    if text.is_empty() {
        return "{}".to_owned();
    }
    let mut out = String::from("\\text{");
    for c in text.chars() {
        match c {
            '{' | '}' | '$' | '%' | '#' | '&' | '_' => {
                out.push('\\');
                out.push(c);
            }
            '\\' => out.push_str("\\textbackslash{}"),
            c => out.push(c),
        }
    }
    out.push('}');
    out
}
