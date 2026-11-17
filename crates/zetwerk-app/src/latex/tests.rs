//! The converter by tables: what LaTeX becomes, that Typst reads all of
//! it (parsed, and typeset), that what it does not know is kept and
//! said, and that Typst written as LaTeX comes back as it was.

use std::fmt::Write;
use std::path::PathBuf;

use typst::WorldExt;
use zetwerk_typst::{Environment, FontSet, Project};

use super::*;

/// The equations of `equations` that Typst refuses when they are
/// typeset, each with why.
fn refused(equations: &[String]) -> Vec<(String, String)> {
    let mut text = String::new();
    let mut starts = Vec::new();
    for equation in equations {
        starts.push(text.len());
        let _ = write!(text, "$ {equation} $\n\n");
    }
    let env = Environment::new(FontSet::Embedded);
    let project = Project::in_memory(env, PathBuf::from("/"), "main.typ", text)
        .unwrap_or_else(|| panic!("a main file"));
    let world = project.snapshot();
    let compiled = zetwerk_typst::compile(&world);
    compiled
        .errors
        .iter()
        .map(|error| {
            let at = world.range(error.span).map_or(0, |range| range.start);
            let index = starts.iter().rposition(|start| *start <= at).unwrap_or(0);
            (equations[index].clone(), error.message.to_string())
        })
        .collect()
}

/// Whether Typst parses `typst` as the inside of an equation without a
/// complaint.
fn parses(typst: &str) -> bool {
    let root = typst::syntax::parse(&format!("${typst}$"));
    let errors = root.errors_and_warnings().0;
    if !errors.is_empty() {
        eprintln!("{typst:?}: {errors:?}");
    }
    errors.is_empty()
}

#[test]
fn every_name_is_one_typst_knows() {
    let mut equations: Vec<String> = SYMBOLS
        .iter()
        .map(|(latex, typst)| format!("{typst} \"{latex}\""))
        .collect();
    for (latex, shape) in COMMANDS {
        let typst = to_typst(&format!("\\{latex}{{x}}{{y}}")).text;
        equations.push(format!("{typst} \"{shape:?}\""));
    }
    for (name, _) in table::ENVIRONMENTS {
        let typst = to_typst(&format!(
            "\\begin{{{name}}} a & b \\\\ c & d \\end{{{name}}}"
        ))
        .text;
        equations.push(typst);
    }
    for (latex, _) in table::COLOURS {
        equations.push(to_typst(&format!("\\textcolor{{{latex}}}{{x}}")).text);
    }
    assert_eq!(refused(&equations), Vec::new());
}

/// LaTeX, and the Typst it becomes.
const CONVERTED: &[(&str, &str)] = &[
    ("", ""),
    ("\\alpha", "alpha"),
    (
        "\\epsilon \\varepsilon \\phi \\varphi",
        "epsilon.alt epsilon phi.alt phi",
    ),
    ("\\infty", "infinity"),
    ("a \\leq b", "a <= b"),
    ("x \\to 0", "x -> 0"),
    ("a \\cdot b \\times c", "a dot b times c"),
    ("\\partial f", "partial f"),
    ("\\nabla \\cdot F", "nabla dot F"),
    ("\\mathbb{R}", "RR"),
    ("\\mathbb{R}^n", "RR^n"),
    ("\\mathbb{1}", "bb(1)"),
    ("1, \\ldots, n", "1, dots.h, n"),
    ("xy", "x y"),
    ("2n", "2n"),
    ("x2", "x 2"),
    ("3.14", "3.14"),
    ("\\frac{a}{b}", "a/b"),
    ("\\frac12", "1/2"),
    ("\\dfrac{a+b}{c}", "(a + b)/c"),
    (
        "\\frac{\\partial u}{\\partial t} = \\alpha \\nabla^2 u",
        "(partial u)/(partial t) = alpha nabla^2 u",
    ),
    ("\\frac{\\frac{a}{b}}{c}", "(a/b)/c"),
    ("\\frac{x^2}{2}", "x^2/2"),
    ("x^{2n}", "x^(2n)"),
    ("x^23", "x^2 3"),
    ("x^{-1}", "x^(- 1)"),
    ("x_{i,j}", "x_(i, j)"),
    ("x_i^2", "x_i^2"),
    ("x^\\frac12", "x^(1/2)"),
    ("e^{i\\pi} + 1 = 0", "e^(i pi) + 1 = 0"),
    ("{}^{14}C", "\"\"^14 C"),
    ("^2", "\"\"^2"),
    ("x'", "x'"),
    ("f''(x)", "f''(x)"),
    ("\\sqrt{x}", "sqrt(x)"),
    ("\\sqrt[n]{x}", "root(n, x)"),
    ("\\sqrt{}", "sqrt(\"\")"),
    ("\\binom{n}{k}", "binom(n, k)"),
    ("\\vec{v}", "arrow(v)"),
    (
        "\\hat{x} \\tilde{y} \\bar{z} \\dot{w}",
        "hat(x) tilde(y) overline(z) dot(w)",
    ),
    ("\\overline{AB}", "overline(A B)"),
    ("\\underline{x}", "underline(x)"),
    ("\\mathbf{v}", "bold(v)"),
    ("\\mathrm{d}x", "upright(d) x"),
    ("\\mathcal{L}", "cal(L)"),
    ("\\text{if } x > 0", "\"if \" x > 0"),
    ("\\text{a \"b\" \\\\ c}", "\"a \\\"b\\\"   c\""),
    ("\\text{for $x>0$}", "\"for \" x > 0"),
    ("\\operatorname{tr} A", "op(\"tr\") A"),
    (
        "\\operatorname*{argmax}_x",
        "op(\"argmax\", limits: #true)_x",
    ),
    ("\\left( \\frac{a}{b} \\right)", "(a/b)"),
    ("\\left\\{ x \\right\\}", "{x}"),
    ("\\left| x \\right|", "abs(x)"),
    ("\\left\\| x \\right\\|", "norm(x)"),
    ("\\left\\langle u, v \\right\\rangle", "⟨u, v⟩"),
    ("\\left. f \\right|_0^1", "lr(f bar.v)_0^1"),
    ("\\left( x", "lr(paren.l x)"),
    ("\\big( x \\big)", "(x)"),
    (
        "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}",
        "mat(a, b; c, d)",
    ),
    (
        "\\begin{bmatrix} 1 & 0 \\\\ 0 & 1 \\\\ \\end{bmatrix}",
        "mat(delim: \"[\", 1, 0; 0, 1)",
    ),
    (
        "\\begin{array}{cc} 1 & 2 \\end{array}",
        "mat(delim: #none, 1, 2)",
    ),
    (
        "|x| = \\begin{cases} x & x \\geq 0 \\\\ -x & x < 0 \\end{cases}",
        "| x | = cases(x & x >= 0, - x & x < 0)",
    ),
    (
        "\\begin{aligned} a &= b \\\\ &= c \\end{aligned}",
        "a & = b \\\n& = c",
    ),
    ("a \\\\ b", "a \\\nb"),
    ("\\sum_{i=1}^{n} i^2", "sum_(i = 1)^n i^2"),
    ("\\prod_{k} a_k", "product_k a_k"),
    ("\\int_0^1 f(x)\\,dx", "integral_0^1 f(x) thin d x"),
    (
        "\\lim_{x \\to 0} \\frac{\\sin x}{x} = 1",
        "lim_(x -> 0) (sin x)/x = 1",
    ),
    ("a \\neq b", "a != b"),
    ("A \\subseteq B", "A subset.eq B"),
    ("\\forall x \\in X", "forall x in X"),
    ("x \\notin A", "x in.not A"),
    ("x \\not\\in A", "x in.not A"),
    ("a \\not= b", "a != b"),
    ("\\not\\sqsubset", "cancel(subset.sq)"),
    ("\\{1, 2\\}", "brace.l 1, 2 brace.r"),
    ("a \\bmod b", "a mod b"),
    ("a/b", "a \\/ b"),
    ("50\\%", "50 %"),
    ("\\$5", "\\$ 5"),
    ("a\\_b", "a \\_ b"),
    ("\"x\" \\# @", "\\\" x \\\" \\# \\@"),
    ("a\\quad b \\qquad c \\; d \\! e", "a quad b wide c med d e"),
    ("a~b", "a space.nobreak b"),
    ("\\overbrace{a+b}^{n}", "overbrace(a + b, n)"),
    ("\\underbrace{a+b}_{n}", "underbrace(a + b, n)"),
    ("\\xrightarrow{f}", "limits(arrow.r.long)^f"),
    ("\\overset{!}{=}", "limits(=)^(!)"),
    ("\\stackrel{def}{=}", "limits(=)^(d e f)"),
    ("\\color{red} x + y", "text(x + y, fill: #red)"),
    ("\\textcolor{blue}{y}", "text(y, fill: #blue)"),
    ("\\boxed{x}", "#box($x$, stroke: 0.5pt, inset: 3pt)"),
    ("\\phantom{x}", "#hide($x$)"),
    ("{a \\over b}", "a/b"),
    ("{n \\choose k}", "binom(n, k)"),
    ("x \\pmod{n}", "x(mod n)"),
    ("a \\displaystyle b \\limits", "a b"),
    ("x \\label{eq:1}", "x"),
    ("\\frac{a,b}{c}", "(a, b)/c"),
    ("\\sqrt{a,b}", "sqrt(a \\, b)"),
    ("% a comment\nx", "x"),
    ("α β", "α β"),
    ("√2", "\\√ 2"),
    ("\\", ""),
    ("}", ""),
    ("{", ""),
    ("x^", "x^\"\""),
];

#[test]
fn latex_becomes_the_typst_that_means_it() {
    let mut wrong = Vec::new();
    for (latex, typst) in CONVERTED {
        let converted = to_typst(latex);
        if converted.text != *typst {
            wrong.push(format!(
                "{latex:?}\n  is {:?}\n  not {typst:?}",
                converted.text
            ));
        }
        assert!(converted.kept.is_empty(), "{latex}: {:?}", converted.kept);
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn every_conversion_is_read_and_typeset_by_typst() {
    let outputs: Vec<String> = CONVERTED
        .iter()
        .map(|(latex, _)| to_typst(latex).text)
        .collect();
    for output in &outputs {
        assert!(parses(output), "{output:?} is not read");
    }
    assert_eq!(refused(&outputs), Vec::new());
}

#[test]
fn what_is_not_known_is_kept_as_text_and_said() {
    let converted = to_typst("\\foo x + \\bar{y} \\foo");
    assert_eq!(converted.text, "\"\\\\foo\" x + overline(y) \"\\\\foo\"");
    assert_eq!(converted.kept, vec!["\\foo".to_owned()]);
    let converted = to_typst("\\begin{weird} a & b \\end{weird}");
    assert_eq!(converted.text, "a & b");
    assert_eq!(converted.kept, vec!["\\begin{weird}".to_owned()]);
    let converted = to_typst("\\color{puce} x");
    assert_eq!(converted.text, "x");
    assert_eq!(converted.kept, vec!["the colour puce".to_owned()]);
    assert_eq!(refused(&[to_typst("\\foo x").text]), Vec::new());
}

/// A random LaTeX-ish text: pieces of LaTeX, in any order.
#[allow(clippy::too_many_lines, reason = "one line per piece")]
fn random_latex(seed: &mut u64, length: usize) -> String {
    const PIECES: &[&str] = &[
        "\\frac",
        "\\sqrt",
        "{",
        "}",
        "{",
        "}",
        "^",
        "_",
        "x",
        "y",
        "2",
        "10",
        "+",
        "-",
        "=",
        "(",
        ")",
        "[",
        "]",
        "|",
        "\\left",
        "\\right",
        "\\left(",
        "\\right)",
        "\\alpha",
        "\\\\",
        "&",
        "\\begin{pmatrix}",
        "\\end{pmatrix}",
        "\\begin{cases}",
        "\\end{cases}",
        "\\text{",
        "\\mathbb",
        "R",
        "\\foo",
        "\\",
        "'",
        ",",
        ";",
        ".",
        ":",
        "!",
        "/",
        "*",
        "~",
        "<",
        ">",
        "\\,",
        "\\{",
        "\\}",
        "\\|",
        "\"",
        "#",
        "$",
        "%",
        "\n",
        " ",
        "\\not",
        "\\over",
        "\\operatorname",
        "\\color",
        "red",
        "\\sum",
        "\\int",
        "\\lim",
        "\\to",
        "\\cdot",
        "\\hat",
        "\\overbrace",
        "\\xrightarrow",
        "\\sqrt[",
        "α",
        "√",
        "\\begin{array}",
        "{cc}",
        "\\end{array}",
        "\\label",
        "\\mathrm",
        "d",
        "@",
        "`",
        "\\$",
        "\\#",
        "\\_",
        "\\&",
        "-",
        ">",
        "<",
        "=",
        "|",
        "[",
        "]",
    ];
    let mut text = String::new();
    for _ in 0..length {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let index = usize::try_from(*seed >> 33).unwrap_or(0) % PIECES.len();
        text.push_str(PIECES[index]);
    }
    text
}

#[test]
fn whatever_it_is_given_typst_reads_what_it_writes() {
    let mut seed = 7;
    let mut outputs = Vec::new();
    for round in 0..3000 {
        let latex = random_latex(&mut seed, 1 + round % 40);
        let typst = to_typst(&latex).text;
        assert!(
            parses(&typst),
            "{latex:?} became {typst:?}, which is not read"
        );
        outputs.push(typst);
    }
    let refused = refused(&outputs);
    assert!(refused.is_empty(), "{refused:#?}");
    // Nor does depth bring it down.
    let deep = format!("{}x{}", "{".repeat(5000), "}".repeat(5000));
    assert!(parses(&to_typst(&deep).text));
    let deep = "\\frac".repeat(3000);
    assert!(parses(&to_typst(&deep).text));
    let deep = "x^".repeat(3000);
    assert!(parses(&to_typst(&deep).text));
}

/// Typst as the converter writes it, which goes to LaTeX and comes back
/// the same.
const ROUND: &[&str] = &[
    "a/b",
    "x^2 + y^2 = z^2",
    "sum_(i = 1)^n i^2",
    "sqrt(x)",
    "root(3, x)",
    "alpha + beta",
    "(partial u)/(partial t) = alpha nabla^2 u",
    "binom(n, k)",
    "hat(x)",
    "mat(a, b; c, d)",
    "mat(delim: \"[\", 1, 0; 0, 1)",
    "cases(x & x >= 0, - x & x < 0)",
    "RR^n",
    "arrow(v) dot arrow(w)",
    "abs(x)",
    "norm(x)",
    "integral_0^1 f(x) thin d x",
    "a <= b",
    "A subset.eq B",
    "\"if \" x > 0",
    "op(\"tr\") A",
    "f'(x)",
    "e^(i pi)",
    "lim_(x -> 0) (sin x)/x = 1",
    "x_(i, j)",
    "upright(d) x",
    "cal(L)",
    "bold(v)",
    "overline(x)",
    "x in.not A",
    "dots.h",
    "a != b",
    "forall epsilon > 0 exists delta > 0",
    "x -> infinity",
    "overbrace(a + b, n)",
    "\"\\\\foo\" x",
];

#[test]
fn typst_written_as_latex_comes_back_as_it_was() {
    let mut wrong = Vec::new();
    for typst in ROUND {
        let latex = to_latex(typst);
        assert!(latex.kept.is_empty(), "{typst}: {:?}", latex.kept);
        let back = to_typst(&latex.text).text;
        if back != *typst {
            wrong.push(format!("{typst:?}\n  as {:?}\n  is {back:?}", latex.text));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn typst_is_written_as_latex() {
    for (typst, latex) in [
        ("frac(a, b)", "\\frac{a}{b}"),
        ("alpha", "\\alpha"),
        ("x <= y", "x \\leq y"),
        ("α + 1", "\\alpha + 1"),
        ("x^2", "x^{2}"),
        ("vec(1, 2)", "\\begin{pmatrix} 1 \\\\ 2 \\end{pmatrix}"),
        (
            "a & = b \\\n& = c",
            "\\begin{aligned}\na & = b \\\\\n& = c\n\\end{aligned}",
        ),
        ("\"a_b\"", "\\text{a\\_b}"),
        ("dif x", "\\mathrm{d} x"),
        ("oo", "\\infty"),
        ("(a/b)", "\\left( \\frac{a}{b} \\right)"),
    ] {
        assert_eq!(to_latex(typst).text, latex, "{typst}");
    }
    // Code has no LaTeX: it is said.
    let code = to_latex("x + #calc.pi");
    assert_eq!(code.kept, vec!["#calc.pi".to_owned()]);
}

#[test]
fn latex_is_told_from_typst() {
    for latex in ["\\alpha", "x^{2}", "a_{ij}", "a \\\\ b", "\\{x\\}", "a\\,b"] {
        assert!(looks_like_latex(latex), "{latex}");
    }
    for typst in ["alpha", "x^2", "frac(a, b)", "\\u{1F600}", "a \\$", "\"a\""] {
        assert!(!looks_like_latex(typst), "{typst}");
    }
}

#[test]
fn mathematics_in_running_text_becomes_equations() {
    let cases = [
        (
            "Let $\\alpha > 0$ and \\(x\\) be real.",
            "Let $alpha > 0$ and $x$ be real.",
        ),
        ("Then $$\\frac{a}{b}$$ holds.", "Then $ a/b $ holds."),
        ("Then \\[ x^{2} \\] holds.", "Then $ x^2 $ holds."),
        (
            "\\begin{equation}\nE = mc^2\n\\end{equation}",
            "$ E = m c^2 $",
        ),
        (
            "\\begin{align*} a &= b \\\\ &= c \\end{align*}",
            "$ a & = b \\\n& = c $",
        ),
        ("It costs \\$5, $\\alpha$.", "It costs \\$5, $alpha$."),
    ];
    for (latex, typst) in cases {
        let converted = markup_to_typst(latex);
        assert_eq!(
            converted.map(|converted| converted.text).as_deref(),
            Some(typst),
            "{latex}"
        );
    }
    // Typst stays Typst.
    assert_eq!(markup_to_typst("A $x^2$ here, and $alpha$."), None);
    assert_eq!(markup_to_typst("No mathematics."), None);
}
