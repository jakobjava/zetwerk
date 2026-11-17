//! The names of LaTeX and what they are in Typst: symbols, the names of
//! functions set upright (`\sin`), and the commands that take arguments.
//!
//! Typst's names are not trusted to be right because they are written
//! here: a test typesets every one of them (`every_name_is_one_typst_knows`).

use std::collections::HashMap;
use std::sync::OnceLock;

/// LaTeX names without arguments and what Typst writes for them: the
/// name of a symbol, or a shorthand (`<=`). The first of two LaTeX names
/// for one Typst name is the one Typst is written back as.
pub const SYMBOLS: &[(&str, &str)] = &[
    // Greek, small. LaTeX's `\epsilon` and `\phi` are the forms Typst
    // calls `.alt`; its `\varepsilon` and `\varphi` are Typst's plain.
    ("alpha", "alpha"),
    ("beta", "beta"),
    ("gamma", "gamma"),
    ("delta", "delta"),
    ("epsilon", "epsilon.alt"),
    ("varepsilon", "epsilon"),
    ("zeta", "zeta"),
    ("eta", "eta"),
    ("theta", "theta"),
    ("vartheta", "theta.alt"),
    ("iota", "iota"),
    ("kappa", "kappa"),
    ("varkappa", "kappa.alt"),
    ("lambda", "lambda"),
    ("mu", "mu"),
    ("nu", "nu"),
    ("xi", "xi"),
    ("omicron", "omicron"),
    ("pi", "pi"),
    ("varpi", "pi.alt"),
    ("rho", "rho"),
    ("varrho", "rho.alt"),
    ("sigma", "sigma"),
    ("varsigma", "sigma.alt"),
    ("tau", "tau"),
    ("upsilon", "upsilon"),
    ("phi", "phi.alt"),
    ("varphi", "phi"),
    ("chi", "chi"),
    ("psi", "psi"),
    ("omega", "omega"),
    ("digamma", "digamma"),
    // Greek, capital.
    ("Gamma", "Gamma"),
    ("Delta", "Delta"),
    ("Theta", "Theta"),
    ("Lambda", "Lambda"),
    ("Xi", "Xi"),
    ("Pi", "Pi"),
    ("Sigma", "Sigma"),
    ("Upsilon", "Upsilon"),
    ("Phi", "Phi"),
    ("Psi", "Psi"),
    ("Omega", "Omega"),
    // Hebrew and letter-likes.
    ("aleph", "aleph"),
    ("beth", "beth"),
    ("gimel", "gimel"),
    ("daleth", "daleth"),
    ("ell", "ell"),
    ("hbar", "planck"),
    ("hslash", "planck"),
    ("wp", "pee"),
    ("Re", "Re"),
    ("Im", "Im"),
    ("imath", "dotless.i"),
    ("jmath", "dotless.j"),
    ("mho", "Omega.inv"),
    ("Angstrom", "angstrom"),
    // Calculus and logic.
    ("infty", "infinity"),
    ("partial", "partial"),
    ("nabla", "nabla"),
    ("forall", "forall"),
    ("exists", "exists"),
    ("nexists", "exists.not"),
    ("neg", "not"),
    ("lnot", "not"),
    ("top", "top"),
    ("bot", "bot"),
    ("emptyset", "emptyset"),
    ("varnothing", "nothing"),
    ("therefore", "therefore"),
    ("because", "because"),
    ("complement", "complement"),
    ("prime", "prime"),
    ("backprime", "prime.rev"),
    ("angle", "angle"),
    ("measuredangle", "angle.arc"),
    ("degree", "degree"),
    ("checkmark", "checkmark"),
    ("flat", "flat"),
    ("natural", "natural"),
    ("sharp", "sharp"),
    ("S", "section"),
    ("P", "pilcrow"),
    ("dag", "dagger"),
    ("ddag", "dagger.double"),
    ("copyright", "copyright"),
    ("pounds", "pound"),
    ("triangle", "triangle.stroked.t"),
    ("Box", "square.stroked"),
    ("square", "square.stroked"),
    ("blacksquare", "square.filled"),
    ("lozenge", "lozenge.stroked"),
    ("blacklozenge", "lozenge.filled"),
    ("bigstar", "star.filled"),
    // Operators.
    ("pm", "plus.minus"),
    ("mp", "minus.plus"),
    ("times", "times"),
    ("div", "div"),
    ("cdot", "dot"),
    ("cdotp", "dot.c"),
    ("ast", "ast.op"),
    ("star", "star.op"),
    ("circ", "compose"),
    ("bullet", "bullet"),
    ("setminus", "without"),
    ("smallsetminus", "without"),
    ("cup", "union"),
    ("cap", "inter"),
    ("sqcup", "union.sq"),
    ("sqcap", "inter.sq"),
    ("uplus", "union.plus"),
    ("wedge", "and"),
    ("land", "and"),
    ("vee", "or"),
    ("lor", "or"),
    ("oplus", "plus.o"),
    ("ominus", "minus.o"),
    ("otimes", "times.o"),
    ("oslash", "slash.o"),
    ("odot", "dot.o"),
    ("dagger", "dagger"),
    ("ddagger", "dagger.double"),
    ("wr", "wreath"),
    ("bigcirc", "circle.stroked.big"),
    ("diamond", "diamond.stroked.small"),
    ("triangleleft", "triangle.stroked.small.l"),
    ("triangleright", "triangle.stroked.small.r"),
    ("bigtriangleup", "triangle.stroked.t"),
    ("bigtriangledown", "triangle.stroked.b"),
    ("ltimes", "times.l"),
    ("rtimes", "times.r"),
    // Large operators.
    ("sum", "sum"),
    ("prod", "product"),
    ("coprod", "product.co"),
    ("int", "integral"),
    ("iint", "integral.double"),
    ("iiint", "integral.triple"),
    ("iiiint", "integral.quad"),
    ("oint", "integral.cont"),
    ("oiint", "integral.surf"),
    ("bigcup", "union.big"),
    ("bigcap", "inter.big"),
    ("bigsqcup", "union.sq.big"),
    ("bigvee", "or.big"),
    ("bigwedge", "and.big"),
    ("bigoplus", "plus.o.big"),
    ("bigotimes", "times.o.big"),
    ("bigodot", "dot.o.big"),
    ("biguplus", "union.plus.big"),
    // Relations.
    ("leq", "<="),
    ("le", "<="),
    ("geq", ">="),
    ("ge", ">="),
    ("neq", "!="),
    ("ne", "!="),
    ("leqslant", "lt.eq.slant"),
    ("geqslant", "gt.eq.slant"),
    ("leqq", "lt.equiv"),
    ("geqq", "gt.equiv"),
    ("nleq", "lt.eq.not"),
    ("ngeq", "gt.eq.not"),
    ("nless", "lt.not"),
    ("ngtr", "gt.not"),
    ("lneq", "lt.neq"),
    ("gneq", "gt.neq"),
    ("lesssim", "lt.tilde"),
    ("gtrsim", "gt.tilde"),
    ("lessgtr", "lt.gt"),
    ("gtrless", "gt.lt"),
    ("ll", "<<"),
    ("gg", ">>"),
    ("lll", "lt.triple"),
    ("ggg", "gt.triple"),
    ("prec", "prec"),
    ("succ", "succ"),
    ("preceq", "prec.eq"),
    ("succeq", "succ.eq"),
    ("preccurlyeq", "prec.curly.eq"),
    ("succcurlyeq", "succ.curly.eq"),
    ("sim", "tilde.op"),
    ("nsim", "tilde.not"),
    ("simeq", "tilde.eq"),
    ("cong", "tilde.equiv"),
    ("ncong", "tilde.equiv.not"),
    ("approx", "approx"),
    ("napprox", "approx.not"),
    ("approxeq", "approx.eq"),
    ("equiv", "equiv"),
    ("nequiv", "equiv.not"),
    ("propto", "prop"),
    ("varpropto", "prop"),
    ("asymp", "asymp"),
    ("doteq", "eq.dot"),
    ("triangleq", "eq.delta"),
    ("coloneqq", ":="),
    ("eqqcolon", "eq.colon"),
    ("models", "models"),
    ("vdash", "tack.r"),
    ("dashv", "tack.l"),
    ("perp", "perp"),
    ("parallel", "parallel"),
    ("nparallel", "parallel.not"),
    ("mid", "divides"),
    ("nmid", "divides.not"),
    ("in", "in"),
    ("ni", "in.rev"),
    ("owns", "in.rev"),
    ("notin", "in.not"),
    ("subset", "subset"),
    ("supset", "supset"),
    ("subseteq", "subset.eq"),
    ("supseteq", "supset.eq"),
    ("subsetneq", "subset.neq"),
    ("supsetneq", "supset.neq"),
    ("nsubseteq", "subset.eq.not"),
    ("nsupseteq", "supset.eq.not"),
    ("sqsubset", "subset.sq"),
    ("sqsupset", "supset.sq"),
    ("sqsubseteq", "subset.eq.sq"),
    ("sqsupseteq", "supset.eq.sq"),
    ("smile", "smile"),
    ("frown", "frown"),
    ("bowtie", "bowtie.stroked"),
    ("colon", "colon"),
    // Arrows.
    ("to", "->"),
    ("rightarrow", "->"),
    ("gets", "<-"),
    ("leftarrow", "<-"),
    ("leftrightarrow", "<->"),
    ("Rightarrow", "=>"),
    ("Leftarrow", "arrow.l.double"),
    ("Leftrightarrow", "<=>"),
    ("longrightarrow", "-->"),
    ("longleftarrow", "<--"),
    ("longleftrightarrow", "<-->"),
    ("implies", "==>"),
    ("Longrightarrow", "==>"),
    ("impliedby", "<=="),
    ("Longleftarrow", "<=="),
    ("iff", "<==>"),
    ("Longleftrightarrow", "<==>"),
    ("mapsto", "|->"),
    ("longmapsto", "mapsto.long"),
    ("uparrow", "arrow.t"),
    ("downarrow", "arrow.b"),
    ("updownarrow", "arrow.t.b"),
    ("Uparrow", "arrow.t.double"),
    ("Downarrow", "arrow.b.double"),
    ("Updownarrow", "arrow.t.b.double"),
    ("nearrow", "arrow.tr"),
    ("searrow", "arrow.br"),
    ("nwarrow", "arrow.tl"),
    ("swarrow", "arrow.bl"),
    ("hookrightarrow", "arrow.r.hook"),
    ("hookleftarrow", "arrow.l.hook"),
    ("leadsto", "arrow.r.squiggly"),
    ("rightsquigarrow", "arrow.r.squiggly"),
    ("twoheadrightarrow", "arrow.r.twohead"),
    ("twoheadleftarrow", "arrow.l.twohead"),
    ("rightarrowtail", "arrow.r.tail"),
    ("leftarrowtail", "arrow.l.tail"),
    ("Rrightarrow", "arrow.r.triple"),
    ("Lleftarrow", "arrow.l.triple"),
    ("nrightarrow", "arrow.r.not"),
    ("nleftarrow", "arrow.l.not"),
    ("nRightarrow", "arrow.r.double.not"),
    ("rightrightarrows", "arrows.rr"),
    ("leftleftarrows", "arrows.ll"),
    ("rightleftarrows", "arrows.rl"),
    ("leftrightarrows", "arrows.lr"),
    ("circlearrowleft", "arrow.ccw"),
    ("circlearrowright", "arrow.cw"),
    ("curvearrowleft", "arrow.ccw.half"),
    ("curvearrowright", "arrow.cw.half"),
    ("rightharpoonup", "harpoon.rt"),
    ("rightharpoondown", "harpoon.rb"),
    ("leftharpoonup", "harpoon.lt"),
    ("leftharpoondown", "harpoon.lb"),
    ("upharpoonleft", "harpoon.tl"),
    ("upharpoonright", "harpoon.tr"),
    ("downharpoonleft", "harpoon.bl"),
    ("downharpoonright", "harpoon.br"),
    ("rightleftharpoons", "harpoons.rtlb"),
    ("leftrightharpoons", "harpoons.ltrb"),
    // Dots.
    ("ldots", "dots.h"),
    ("dots", "dots.h"),
    ("dotsc", "dots.h"),
    ("dotso", "dots.h"),
    ("dotsb", "dots.c"),
    ("dotsm", "dots.c"),
    ("dotsi", "dots.c"),
    ("cdots", "dots.c"),
    ("vdots", "dots.v"),
    ("ddots", "dots.down"),
    ("iddots", "dots.up"),
    // Delimiters, where they are not those of `\left` and `\right`.
    ("langle", "chevron.l"),
    ("rangle", "chevron.r"),
    ("lfloor", "floor.l"),
    ("rfloor", "floor.r"),
    ("lceil", "ceil.l"),
    ("rceil", "ceil.r"),
    ("vert", "bar.v"),
    ("lvert", "bar.v"),
    ("rvert", "bar.v"),
    ("Vert", "bar.v.double"),
    ("lVert", "bar.v.double"),
    ("rVert", "bar.v.double"),
    ("lbrace", "brace.l"),
    ("rbrace", "brace.r"),
    ("lbrack", "bracket.l"),
    ("rbrack", "bracket.r"),
    ("llbracket", "bracket.l.stroked"),
    ("rrbracket", "bracket.r.stroked"),
    ("ulcorner", "corner.l.t"),
    ("urcorner", "corner.r.t"),
    ("llcorner", "corner.l.b"),
    ("lrcorner", "corner.r.b"),
    ("backslash", "backslash"),
    // Spaces.
    ("quad", "quad"),
    ("qquad", "wide"),
    ("thinspace", "thin"),
    ("medspace", "med"),
    ("thickspace", "thick"),
    ("enspace", "space.en"),
    // Names of functions, set upright.
    ("arccos", "arccos"),
    ("arcsin", "arcsin"),
    ("arctan", "arctan"),
    ("arg", "arg"),
    ("cos", "cos"),
    ("cosh", "cosh"),
    ("cot", "cot"),
    ("coth", "coth"),
    ("csc", "csc"),
    ("deg", "deg"),
    ("det", "det"),
    ("dim", "dim"),
    ("exp", "exp"),
    ("gcd", "gcd"),
    ("lcm", "lcm"),
    ("hom", "hom"),
    ("inf", "inf"),
    ("ker", "ker"),
    ("lg", "lg"),
    ("lim", "lim"),
    ("liminf", "liminf"),
    ("limsup", "limsup"),
    ("ln", "ln"),
    ("log", "log"),
    ("max", "max"),
    ("min", "min"),
    ("Pr", "Pr"),
    ("sec", "sec"),
    ("sin", "sin"),
    ("sinh", "sinh"),
    ("sup", "sup"),
    ("tan", "tan"),
    ("tanh", "tanh"),
    ("mod", "mod"),
    ("bmod", "mod"),
];

/// What a LaTeX command with arguments becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// A Typst function of as many arguments: `hat(x)`, `binom(n, k)`.
    Call(&'static str, usize),
    /// A fraction: `frac(a, b)` typed, `a/b` converted.
    Frac,
    /// `\sqrt`, and with `[n]` before its argument a root of `n`.
    Sqrt,
    /// Text: `"…"`.
    Text,
    /// A name of a function set upright: `op("…")`.
    Operator,
    /// Something above (`\overset`) or below (`\underset`) another,
    /// which is the second argument.
    Over(bool),
    /// `\overbrace` and `\underbrace`, whose note is the script behind.
    Brace(&'static str),
    /// A colour for what follows, or for the second argument.
    Colour(bool),
    /// Arguments that are said nothing of: `\label`, `\tag`, `\hspace`.
    Dropped(usize),
    /// What shows the argument without its ink: `\phantom`.
    Phantom,
    /// A frame around it: `\boxed`.
    Boxed,
    /// An arrow that stretches under what is written over it.
    Arrow(&'static str),
}

/// The LaTeX commands with arguments, and what they become.
pub const COMMANDS: &[(&str, Shape)] = &[
    ("frac", Shape::Frac),
    ("dfrac", Shape::Frac),
    ("tfrac", Shape::Frac),
    ("cfrac", Shape::Frac),
    ("sqrt", Shape::Sqrt),
    ("binom", Shape::Call("binom", 2)),
    ("dbinom", Shape::Call("binom", 2)),
    ("tbinom", Shape::Call("binom", 2)),
    ("hat", Shape::Call("hat", 1)),
    ("widehat", Shape::Call("hat", 1)),
    ("tilde", Shape::Call("tilde", 1)),
    ("widetilde", Shape::Call("tilde", 1)),
    ("overline", Shape::Call("overline", 1)),
    ("bar", Shape::Call("overline", 1)),
    ("underline", Shape::Call("underline", 1)),
    ("vec", Shape::Call("arrow", 1)),
    ("overrightarrow", Shape::Call("arrow", 1)),
    ("overleftarrow", Shape::Call("arrow.l", 1)),
    ("dot", Shape::Call("dot", 1)),
    ("ddot", Shape::Call("dot.double", 1)),
    ("dddot", Shape::Call("dot.triple", 1)),
    ("acute", Shape::Call("acute", 1)),
    ("grave", Shape::Call("grave", 1)),
    ("breve", Shape::Call("breve", 1)),
    ("check", Shape::Call("caron", 1)),
    ("mathring", Shape::Call("circle", 1)),
    ("mathbf", Shape::Call("bold", 1)),
    ("boldsymbol", Shape::Call("bold", 1)),
    ("bm", Shape::Call("bold", 1)),
    ("mathrm", Shape::Call("upright", 1)),
    ("mathit", Shape::Call("italic", 1)),
    ("mathcal", Shape::Call("cal", 1)),
    ("mathscr", Shape::Call("scr", 1)),
    ("mathbb", Shape::Call("bb", 1)),
    ("mathfrak", Shape::Call("frak", 1)),
    ("mathsf", Shape::Call("sans", 1)),
    ("mathtt", Shape::Call("mono", 1)),
    ("cancel", Shape::Call("cancel", 1)),
    ("bcancel", Shape::Call("cancel", 1)),
    ("xcancel", Shape::Call("cancel", 1)),
    ("text", Shape::Text),
    ("textrm", Shape::Text),
    ("textnormal", Shape::Text),
    ("textup", Shape::Text),
    ("mbox", Shape::Text),
    ("hbox", Shape::Text),
    ("textbf", Shape::Call("bold", 1)),
    ("textit", Shape::Call("italic", 1)),
    ("textsf", Shape::Call("sans", 1)),
    ("texttt", Shape::Call("mono", 1)),
    ("operatorname", Shape::Operator),
    ("overset", Shape::Over(true)),
    ("stackrel", Shape::Over(true)),
    ("underset", Shape::Over(false)),
    ("overbrace", Shape::Brace("overbrace")),
    ("underbrace", Shape::Brace("underbrace")),
    ("textcolor", Shape::Colour(true)),
    ("color", Shape::Colour(false)),
    ("label", Shape::Dropped(1)),
    ("tag", Shape::Dropped(1)),
    ("hspace", Shape::Dropped(1)),
    ("vspace", Shape::Dropped(1)),
    ("phantom", Shape::Phantom),
    ("boxed", Shape::Boxed),
    ("xrightarrow", Shape::Arrow("arrow.r.long")),
    ("xleftarrow", Shape::Arrow("arrow.l.long")),
    ("xLeftarrow", Shape::Arrow("arrow.l.double.long")),
    ("xRightarrow", Shape::Arrow("arrow.r.double.long")),
    ("xmapsto", Shape::Arrow("arrow.r.long.bar")),
];

/// Commands that say nothing Typst needs to hear: sizes, styles, and
/// numbering of LaTeX's own.
pub const SILENT: &[&str] = &[
    "limits",
    "nolimits",
    "displaystyle",
    "textstyle",
    "scriptstyle",
    "scriptscriptstyle",
    "nonumber",
    "notag",
    "mathstrut",
    "strut",
    "allowbreak",
    "relax",
    "left",
    "right",
    "middle",
    "big",
    "Big",
    "bigg",
    "Bigg",
    "bigl",
    "bigr",
    "bigm",
    "Bigl",
    "Bigr",
    "Bigm",
    "biggl",
    "biggr",
    "biggm",
    "Biggl",
    "Biggr",
    "Biggm",
    "negthinspace",
    "negmedspace",
    "negthickspace",
];

/// The LaTeX environments of mathematics and how their cells are laid
/// out in Typst.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// `mat(…)`, with the delimiters given, if not round brackets.
    Matrix(Option<&'static str>),
    /// `cases(…)`.
    Cases,
    /// Lines of an equation, aligned at `&`.
    Lines,
}

/// The environments, by name.
pub const ENVIRONMENTS: &[(&str, Layout)] = &[
    ("pmatrix", Layout::Matrix(None)),
    ("bmatrix", Layout::Matrix(Some("\"[\""))),
    ("Bmatrix", Layout::Matrix(Some("\"{\""))),
    ("vmatrix", Layout::Matrix(Some("\"|\""))),
    ("Vmatrix", Layout::Matrix(Some("\"‖\""))),
    ("matrix", Layout::Matrix(Some("#none"))),
    ("smallmatrix", Layout::Matrix(Some("#none"))),
    ("array", Layout::Matrix(Some("#none"))),
    ("cases", Layout::Cases),
    ("dcases", Layout::Cases),
    ("aligned", Layout::Lines),
    ("align", Layout::Lines),
    ("align*", Layout::Lines),
    ("alignat", Layout::Lines),
    ("alignat*", Layout::Lines),
    ("split", Layout::Lines),
    ("gather", Layout::Lines),
    ("gather*", Layout::Lines),
    ("gathered", Layout::Lines),
    ("multline", Layout::Lines),
    ("multline*", Layout::Lines),
    ("eqnarray", Layout::Lines),
    ("eqnarray*", Layout::Lines),
    ("equation", Layout::Lines),
    ("equation*", Layout::Lines),
    ("displaymath", Layout::Lines),
    ("math", Layout::Lines),
];

/// The colours LaTeX users name, as Typst's.
pub const COLOURS: &[(&str, &str)] = &[
    ("black", "black"),
    ("gray", "gray"),
    ("grey", "gray"),
    ("darkgray", "gray"),
    ("lightgray", "silver"),
    ("white", "white"),
    ("blue", "blue"),
    ("cyan", "aqua"),
    ("teal", "teal"),
    ("purple", "purple"),
    ("violet", "purple"),
    ("magenta", "fuchsia"),
    ("red", "red"),
    ("orange", "orange"),
    ("yellow", "yellow"),
    ("olive", "olive"),
    ("green", "green"),
    ("lime", "lime"),
    ("navy", "navy"),
    ("maroon", "maroon"),
];

/// The delimiters `\left` and `\right` take, by how they are written in
/// LaTeX, and as Typst writes each.
pub const DELIMITERS: &[(&str, &str)] = &[
    ("(", "("),
    (")", ")"),
    ("[", "["),
    ("]", "]"),
    ("\\{", "{"),
    ("\\}", "}"),
    ("\\lbrace", "{"),
    ("\\rbrace", "}"),
    ("|", "|"),
    ("\\vert", "|"),
    ("\\lvert", "|"),
    ("\\rvert", "|"),
    ("\\|", "‖"),
    ("\\Vert", "‖"),
    ("\\lVert", "‖"),
    ("\\rVert", "‖"),
    ("\\langle", "⟨"),
    ("\\rangle", "⟩"),
    ("<", "⟨"),
    (">", "⟩"),
    ("\\lfloor", "⌊"),
    ("\\rfloor", "⌋"),
    ("\\lceil", "⌈"),
    ("\\rceil", "⌉"),
    ("/", "\\/"),
    ("\\backslash", "backslash"),
    (".", ""),
];

/// The LaTeX names of the symbols, as a map.
fn symbols() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| SYMBOLS.iter().copied().collect())
}

/// What the LaTeX name `name` (without its backslash) of something with
/// no arguments is in Typst.
#[must_use]
pub fn symbol(name: &str) -> Option<&'static str> {
    symbols().get(name).copied()
}

/// What the LaTeX command `name` with arguments becomes.
#[must_use]
pub fn command(name: &str) -> Option<Shape> {
    COMMANDS
        .iter()
        .find(|(latex, _)| *latex == name)
        .map(|(_, shape)| *shape)
}

/// How the environment `name` lays out its cells.
#[must_use]
pub fn environment(name: &str) -> Option<Layout> {
    ENVIRONMENTS
        .iter()
        .find(|(latex, _)| *latex == name)
        .map(|(_, layout)| *layout)
}

/// The Typst name of LaTeX's colour `name`.
#[must_use]
pub fn colour(name: &str) -> Option<&'static str> {
    COLOURS
        .iter()
        .find(|(latex, _)| *latex == name)
        .map(|(_, typst)| *typst)
}

/// How Typst writes the delimiter LaTeX writes `latex` for `\left` or
/// `\right`.
#[must_use]
pub fn delimiter(latex: &str) -> Option<&'static str> {
    DELIMITERS
        .iter()
        .find(|(written, _)| *written == latex)
        .map(|(_, typst)| *typst)
}

/// The LaTeX name Typst's `typst` is written back as: the first of the
/// table that stands for it.
#[must_use]
pub fn latex_of(typst: &str) -> Option<&'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    let map = MAP.get_or_init(|| {
        let mut map = HashMap::new();
        for (latex, typst) in SYMBOLS {
            map.entry(*typst).or_insert(*latex);
        }
        map
    });
    map.get(typst).copied()
}
