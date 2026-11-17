//! LaTeX typed on the page, key by key, as somebody who has written
//! mathematics in LaTeX for years types it: after every key the document
//! is typeset, nothing of it refused or shown as written; at the end the
//! equation is the Typst that was meant.

use super::*;

/// Typesets all of it as when typing pauses (see `equations.rs`).
fn settle(viewer: &mut Viewer) {
    let _ = viewer.take_edited();
    for _ in 0..4 {
        compile(viewer);
        if viewer.take_edited().is_none() {
            break;
        }
    }
}

/// `keys` typed on the page into `text` with the cursor first at `at`,
/// one at a time; after each, the document is typeset as it is.
fn typed_on_page(text: &str, at: &str, keys: &str) -> (Viewer, PathBuf) {
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "gsgs");
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(text.find(at).unwrap_or(0));
    }
    for key in keys::parse(keys).unwrap_or_else(|err| panic!("{err}")) {
        viewer.key(key);
        settle(&mut viewer);
        let shown = viewer.text().unwrap_or_default().to_owned();
        let problems = viewer.state().problems;
        assert!(problems.is_empty(), "{keys}: {shown:?}: {problems:?}");
        assert!(
            viewer.written.is_none(),
            "{keys}: {shown:?} is shown as written"
        );
    }
    (viewer, directory)
}

#[test]
fn formulas_typed_as_latex_become_typst_at_every_key() {
    let cases = [
        ("\\frac{a}{b}", "$frac(a, b)$"),
        ("x^{2n}", "$x^(2n)$"),
        ("\\alpha + \\beta = \\gamma", "$alpha + beta = gamma$"),
        ("\\sum_{i=1}^{n} i^2", "$sum_(i=1)^(n) i^2$"),
        ("\\int_0^1 f(x)\\,dx", "$integral_0^1 f(x)thin d x$"),
        ("\\sqrt[3]{x} \\leq \\sqrt{y}", "$root(3, x) <= sqrt(y)$"),
        (
            "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}",
            "$mat( a, b; c, d)$",
        ),
        ("\\left( \\frac{1}{2} \\right)^2", "$( frac(1, 2) )^2$"),
        ("\\mathbb{R}^n \\to \\mathbb{R}", "$RR^n -> RR$"),
        ("\\vec{v} \\cdot \\vec{w}", "$arrow(v) dot arrow(w)$"),
        (
            "\\lim_{x \\to 0} \\frac{\\sin x}{x}",
            "$lim_(x -> 0) frac(sin x, x)$",
        ),
        ("\\text{if } x > 0", "$\"if \" x > 0$"),
        (
            "\\frac{\\partial u}{\\partial t} = \\alpha \\nabla^2 u",
            "$frac(partial u, partial t) = alpha nabla^2 u$",
        ),
        (
            "f(x) = \\begin{cases} 1 & x > 0 \\\\ 0 & \\text{else} \\end{cases}",
            "$f(x) = cases( 1 & x > 0, 0 & \"else\")$",
        ),
    ];
    for (latex, typst) in cases {
        let keys = format!("i${latex}$<Esc>");
        let (viewer, directory) = typed_on_page("Words.\n", ".", &keys);
        let text = viewer.text().unwrap_or_default();
        assert_eq!(text, format!("Words{typst}.\n"), "{latex}");
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn a_backslash_before_the_closing_dollar_breaks_nothing() {
    // It escaped the dollar sign: the equation took in the rest of the
    // paragraph, and was shown in red.
    let (viewer, directory) = typed_on_page("A $x$ b.\n", "$ b", "i\\<Esc>");
    assert_eq!(viewer.text(), Some("A $x$ b.\n"));
    let _ = std::fs::remove_dir_all(directory);
    let (viewer, directory) = typed_on_page("A $x$ b.\n", "$ b", "i\\alpha<Esc>");
    assert_eq!(viewer.text(), Some("A $x alpha$ b.\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_name_nobody_knows_stays_as_typed_and_is_said() {
    let (viewer, directory) = typed_on_page("A $x$ b.\n", "$ b", "i \\foo y<Esc>");
    assert_eq!(viewer.text(), Some("A $x \"\\\\foo\" y$ b.\n"));
    let _ = std::fs::remove_dir_all(directory);
    let (mut viewer, directory) = typed_on_page("A $x$ b.\n", "$ b", "i \\foo");
    press(&mut viewer, " ");
    let message = viewer.message.as_ref().map(|message| message.text.clone());
    assert!(
        message.is_some_and(|message| message.starts_with("\\foo is no LaTeX name")),
        "{:?}",
        viewer.message.as_ref().map(|message| &message.text)
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn equations_are_begun_in_running_text_as_in_latex() {
    let (viewer, directory) = typed_on_page("Words.\n", ".", "i \\(x^2\\) and<Esc>");
    assert_eq!(viewer.text(), Some("Words $x^2$ and.\n"));
    let _ = std::fs::remove_dir_all(directory);
    let (viewer, directory) = typed_on_page("Words.\n", ".", "i $$\\frac{a}{b} = c$$<Esc>");
    assert_eq!(viewer.text(), Some("Words $ a/b = c $.\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn tab_ends_a_name_and_escape_leaves_it_made() {
    let (viewer, directory) = typed_on_page("A $x$ b.\n", "$ b", "i\\frac<Tab>1<Tab>2<Esc>");
    assert_eq!(viewer.text(), Some("A $x frac(1, 2)$ b.\n"));
    let _ = std::fs::remove_dir_all(directory);
    let (viewer, directory) = typed_on_page("A $x$ b.\n", "$ b", "i \\beta<Esc>");
    assert_eq!(viewer.text(), Some("A $x beta$ b.\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn backspace_takes_a_name_just_begun() {
    let (viewer, directory) = typed_on_page("A $x$ b.\n", "$ b", "i\\<BS><Esc>");
    assert_eq!(viewer.text(), Some("A $x$ b.\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn latex_command_makes_typst_of_what_is_latex() {
    let (mut viewer, directory) = editing("A $\\frac{a}{b}$ b.\n\nLet \\(\\alpha\\) be.\n");
    press(&mut viewer, "gsgs");
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(4);
    }
    press(&mut viewer, ":latex<CR>");
    assert_eq!(viewer.text(), Some("A $a/b$ b.\n\nLet \\(\\alpha\\) be.\n"));
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(16);
    }
    press(&mut viewer, ":latex<CR>");
    assert_eq!(viewer.text(), Some("A $a/b$ b.\n\nLet $alpha$ be.\n"));
    // One undo step each.
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("A $a/b$ b.\n\nLet \\(\\alpha\\) be.\n"));
    // Typst is left as it is.
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(4);
    }
    press(&mut viewer, ":latex<CR>");
    assert_eq!(viewer.text(), Some("A $a/b$ b.\n\nLet \\(\\alpha\\) be.\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_equation_is_copied_as_latex() {
    static COPIED: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    let (mut viewer, directory) = editing("A $frac(a, b) <= alpha$ b.\n\n$ sum_(i = 1)^n i $\n");
    viewer.clipboard.write = |text| {
        if let Ok(mut copied) = COPIED.lock() {
            text.clone_into(&mut copied);
        }
        Ok(())
    };
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(4);
    }
    press(&mut viewer, ":latex copy<CR>");
    let copied = COPIED
        .lock()
        .map(|copied| copied.clone())
        .unwrap_or_default();
    assert_eq!(copied, "$\\frac{a}{b} \\leq \\alpha$");
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(32);
    }
    press(&mut viewer, ":copy latex<CR>");
    let copied = COPIED
        .lock()
        .map(|copied| copied.clone())
        .unwrap_or_default();
    assert_eq!(copied, "\\[ \\sum_{i = 1}^{n} i \\]");
    let _ = std::fs::remove_dir_all(directory);
}
