//! `:help`: what a key, a command or a word is for, on the page.
//!
//! The list of all there is to do (`palette`) is the list of what there
//! is help on: every entry of it has a text here, found by its keys, and
//! a test makes sure none lacks one and no text is of an entry that is
//! gone. `:help` alone (and `g?`) puts up that list, narrowed by typing
//! as always, and under it what the chosen entry does on the page (and
//! as written, where that differs), what goes with it, and an example;
//! Enter runs or begins its command, where it has one, as the list
//! always did. `:help dd`, `:help gD`, `:help :pdf`, `:help footnote`,
//! `:help tab` put up the same list with the entry chosen that is about
//! that: by its keys first, then by the words it is about.
//!
//! `K` on something of the document says in one line what it is and in
//! another what can be done with it: an equation, a figure, a cell of a
//! table, a heading, a footnote, a label, a comment, bold or emphasised
//! text, an item of a list, code. On a reference the reference helper
//! says what it refers to (see `refer`), and on anything else Typst says
//! what it knows.
//!
//! Where the list of what can follow a key is up (after `g`, `z`, `]`,
//! …; see `hints`), it says that `?` gives help on these, and `?` puts
//! up the entries of those keys. Not after an operator: `d?` searches
//! backwards, as it always did.

use typst::syntax::{LinkedNode, Side, SyntaxKind};

use super::assist::Target;
use super::{Viewer, palette};
use crate::keys::{Key, KeyPress};

/// What is said of an entry of the list of all there is to do.
pub(super) struct Help {
    /// The keys of the entry, as the list has them: what it is found by.
    pub(super) keys: &'static str,
    /// What it does on the page.
    page: &'static str,
    /// What it does where the text is shown as written (`gs`), where
    /// that is not the same.
    written: &'static str,
    /// What goes with it.
    with: &'static str,
    /// One example.
    example: &'static str,
    /// Other words it is looked for by.
    words: &'static str,
}

/// An entry of [`HELP`].
const fn help(
    keys: &'static str,
    page: &'static str,
    written: &'static str,
    with: &'static str,
    example: &'static str,
    words: &'static str,
) -> Help {
    Help {
        keys,
        page,
        written,
        with,
        example,
        words,
    }
}

/// How wide a line of help is, in characters.
const WIDTH: usize = 78;

/// What there is to say of each entry of the list of all there is to do.
pub(super) const HELP: &[Help] = &[
    help(
        ":tutor  :tutor 4",
        "Opens a lesson: a document of its own, in a scratch place, that says what to do in it. A tick in the margin when a step is done.",
        "",
        "Ctrl-T the keys of the step at hand; :tutor list all lessons",
        ":tutor 5 — the lesson on equations",
        "lesson lessons learn tutorial course teach",
    ),
    help(
        "Ctrl-T  :hint",
        "In a lesson: says in the status line which keys do the step at hand.",
        "",
        ":tutor, :help",
        "Ctrl-T while stuck on a step",
        "stuck hint lesson",
    ),
    help(
        ":help <word>",
        "Puts up this list with the entry chosen that is about a key, a command or a word. Type to narrow it; Enter does what the entry does.",
        "",
        "g? the whole list, K what the thing under the cursor is",
        ":help dd   :help footnote   :help :pdf",
        "help what how",
    ),
    help(
        "h  j  k  l",
        "Left, down, up, right. j and k go by the lines the page shows, across columns and pages; a number before them goes that many (5j).",
        "j and k go by the lines as written; gj and gk by the rows shown.",
        "w b e by words, { } by paragraphs, :numbers",
        "5j — five lines of the page down",
        "move cursor left right up down arrow line",
    ),
    help(
        "w  b  e",
        "w to the beginning of the next word, b back to that of this one or the one before, e to its end. Words are as the page reads them.",
        "",
        "W B E by words with what sticks to them, h l by characters",
        "dw takes the word from the cursor on",
        "word move next previous",
    ),
    help(
        "0  $",
        "To the start, to the end of the line as the page shows it: not of the paragraph.",
        "To the start, to the end of the line as written: that is the paragraph.",
        "I and A type there; { } go by paragraphs",
        "$ then a: type at the end of this line",
        "start end line beginning home",
    ),
    help(
        "{  }",
        "To the paragraph before, to the next.",
        "",
        "dd takes a paragraph, ap is one behind d, c, y",
        "} } — two paragraphs on",
        "paragraph move",
    ),
    help(
        "gg  G",
        "To the beginning of the document, to its end. With a number, to that line (as written).",
        "",
        "]] [[ by pages, Ctrl-O back to where it jumped from",
        "G — to the end",
        "top bottom beginning end first last document",
    ),
    help(
        "f  t  ;  ,",
        "f and a character: to the next of it in the line; t: up to it. ; again, , again backwards. F and T look back.",
        "",
        "dt. deletes up to the full stop",
        "f, then ; — from comma to comma",
        "find character jump line",
    ),
    help(
        "Ctrl-D  Ctrl-U  Ctrl-F",
        "Moves the view, and the cursor with it: half a window down, up; a whole window down (Ctrl-B up).",
        "",
        "]] [[ a page on, back; gg G",
        "Ctrl-D Ctrl-D — a window further",
        "scroll page down up",
    ),
    help(
        "x  X",
        "Takes away the character under the cursor (X: the one before it). On a reference, all of the reference; marks of bold and italic stay.",
        "",
        "u brings it back, dw takes a word, r replaces a character",
        "x on a letter typed twice",
        "delete remove character letter",
    ),
    help(
        "dw  dd  D",
        "d and a motion takes what the motion covers: dw a word, d} to the end of the paragraph, diw the word under the cursor. dd takes the paragraph, D the rest of the line shown.",
        "dd takes the line as written: on a page that is also the paragraph.",
        "p puts back what was taken, u brings it back, c deletes and types",
        "dap — the paragraph and the blank after it",
        "delete remove cut word paragraph line",
    ),
    help(
        "cw  ciw  cc  C",
        "c and a motion: deletes what it covers and begins typing there. ciw the word under the cursor, cc the paragraph, C the rest of the line.",
        "",
        "Esc ends typing, . does it again elsewhere, cgn the next match",
        "ciw then a better word, then Esc",
        "change replace word retype",
    ),
    help(
        "yy  p  P",
        "y and a motion copies (yanks); yy the paragraph. p puts it behind the cursor, P before; a paragraph goes in as a paragraph.",
        "",
        "dd then p moves a paragraph; \"+y to the clipboard; :registers",
        "yy then } then p — the paragraph again, further down",
        "copy paste yank put clipboard duplicate",
    ),
    help(
        "v  V",
        "v selects from the cursor as it moves; V takes whole paragraphs. Then d, c, y, gb, gm, Ctrl-B act on what is selected; Esc leaves it.",
        "V takes whole lines as written.",
        "gv the last selection again, o to its other end, Ctrl-V cells",
        "vee then gb — two words bold",
        "select selection visual mark",
    ),
    help(
        "iw  ap  i$",
        "What a key like d, c, y, gb or v takes after i (in) or a (around): w a word, p a paragraph, $ an equation, ( [ { \" brackets and quotes, * _ bold and emphasised text, h a section, l an item.",
        "",
        "d c y v gb gm",
        "di$ — empties the equation; da$ takes it whole",
        "text object inside around word equation bracket",
    ),
    help(
        ".",
        "Does the last change again, where the cursor is now.",
        "",
        "u undoes it, n goes to the next match, cgn",
        "dw, then w . — the next word too",
        "repeat again dot",
    ),
    help(
        "I  A  O",
        "I types at the start of the line the page shows, A at its end, O begins a paragraph above.",
        "I and A: at the start and end of the line as written: the paragraph.",
        "i a o, Esc",
        "A then a full stop, then Esc",
        "append insert type start end line above",
    ),
    help(
        "Esc",
        "Ends typing; leaves a selection, a list, a command being typed, a panel.",
        "",
        "i a o begin typing again",
        "Esc, then u — what was just typed goes",
        "escape normal stop leave cancel",
    ),
    help(
        "gl  gt  gc",
        "gl and a motion makes the paragraphs it covers items of a list; gt makes text look like code (raw); gc makes it a comment, which the page does not show.",
        "",
        "gb bold, gm italic, g+ heading; o in an item begins the next",
        "glap — this paragraph an item",
        "list bullet item raw code comment monospace",
    ),
    help(
        "i  a  o",
        "Begins typing: i before what the cursor is on, a behind it, o in a new paragraph below. What is typed is set on the page as it comes.",
        "",
        "I A O, Esc ends typing, Ctrl-B bold, # to put things in, $ an equation",
        "o then a sentence, then Esc",
        "type write insert typing text",
    ),
    help(
        "#",
        "While typing: a list of what can be put in — equation, figure, table, footnote, image, quote, list, link, … Type to narrow it, Enter or Tab takes one.",
        "",
        "$ for an equation, @ for a reference, Esc leaves the list",
        "#foot then Enter: a footnote, typed where it stands",
        "insert put figure table footnote image link quote function menu",
    ),
    help(
        "$",
        "While typing: an equation in the line, where the cursor stays to type it. Letters typed together are a product (xy is x times y); ^ raises, _ lowers; Tab goes to the next empty place; Esc or $ leaves it.",
        "",
        "#equation for one by itself, gx shows it as written, x takes an empty part",
        "$ then x^2 + y^2 = z^2",
        "equation math formula maths inline",
    ),
    help(
        "Tab",
        "Takes what is listed or shown faint; else goes to the next empty place: of an equation (a missing exponent, a numerator), of a starter (‹…›), of a table (the next cell; behind the last, a row more).",
        "",
        "Shift-Tab back, Esc, # $ @",
        "in $x^□$, Tab goes into the square",
        "tab next place empty fill complete completion cell",
    ),
    help(
        "Ctrl-B  Ctrl-I",
        "While typing: what comes is bold, italic, until the same key again. On a selection: it becomes bold, italic.",
        "",
        "gb gm with a motion, in normal mode",
        "Ctrl-B, a word, Ctrl-B",
        "bold italic strong emphasis",
    ),
    help(
        "gb  gm  g+",
        "gb bold, gm italic (emphasis), of what a motion covers (gbiw: this word). g+ makes the paragraph a heading at once, a level deeper each time; g- shallower.",
        "",
        "iw ap behind them, Ctrl-B Ctrl-I while typing, gl list",
        "gbiw — this word bold",
        "bold italic heading strong emphasis title format",
    ),
    help(
        "@",
        "While typing: a list of all that can be referred to — sections, figures, tables, equations, sources — by label, title or author. Enter puts the reference in; a thing without a label gets one.",
        "",
        "gd goes to what a reference refers to, K says what it is, :references checks them",
        "@ then fig, then Enter",
        "reference cite citation refer label link cross",
    ),
    help(
        "Enter",
        "In a caption, a footnote or a cell of a table: leaves it, and the cursor is behind it. In running text: a new paragraph.",
        "",
        "Shift-Enter a line break, Esc",
        "Enter at the end of a footnote — on with the sentence",
        "enter return leave caption footnote cell",
    ),
    help(
        "u  Ctrl-R",
        "u takes back the last change, and the page shows where; again and again, to the start. Ctrl-R does it again.",
        "",
        "gh every step to look at, :earlier",
        "u u u — three changes back",
        "undo redo back mistake",
    ),
    help(
        "Ctrl-O  Ctrl-I",
        "Back to where a jump (a search, gg, gd, a list) came from; Ctrl-I on again.",
        "",
        ":jumps, `` back and forth",
        "gd on a reference, then Ctrl-O — back to it",
        "back jump return previous",
    ),
    help(
        "/  ?  n  N",
        "/ and words, then Enter: to where the page reads them; ? looks back. n the next, N the one before; all are marked and counted. \\= first looks in the text as written (code, labels).",
        "",
        "* the word at the cursor, :noh, cgn",
        "/bridge then n n",
        "search find look",
    ),
    help(
        "*  #",
        "Searches for the word under the cursor: * on, # back.",
        "",
        "n N, cgn",
        "* on a name — its next place",
        "search word",
    ),
    help(
        "cgn",
        "Changes the next match of the search, and . changes the one after.",
        "",
        "/ to search, :s to replace all",
        "/colour, cgn color Esc, then . .",
        "replace change search match",
    ),
    help(
        ":s/this/that/  :%s/this/that/g",
        "Replaces what is written: in this line (paragraph), or with % in all. g every one in a line.",
        "",
        "gc ask at each, gn count; cgn one at a time; u takes it all back",
        ":%s/colour/color/g",
        "replace substitute",
    ),
    help(
        ":noh",
        "Hides the marks of the last search until the next one.",
        "",
        "/ n, :hlsearch",
        "/word, Enter, then :noh",
        "search highlight",
    ),
    help(
        ":hlsearch",
        "Marks all that a search finds, or not.",
        "",
        ":noh",
        ":hlsearch",
        "search highlight",
    ),
    help(
        ":feedback",
        "Shows what is yanked and put for a moment, and says what deleting took; or not.",
        "",
        "y p d",
        ":feedback",
        "flash yank",
    ),
    help(
        ":s/x/y/gc  :s/x/y/gn",
        "c asks at each match whether to replace it; n only counts them.",
        "",
        ":s",
        ":%s/the/a/gn — how many there are",
        "replace confirm count",
    ),
    help(
        "@:  &  g&",
        "@: the last command line again; & the last :s again here; g& on all lines.",
        "",
        "q: the command lines entered",
        ":s/a/b/ then j &",
        "repeat command",
    ),
    help(
        "q:  q/",
        "The command lines entered, the searches made: to take one again.",
        "",
        "@:",
        "q: then Enter on the one wanted",
        "history command",
    ),
    help(
        ":normal  :g/x/d  :v/x/d",
        ":normal types keys on each line; :g/x/d deletes the lines (paragraphs) where x is, :v those where it is not.",
        "",
        ":s, :sort",
        ":g/TODO/d",
        "global lines",
    ),
    help(
        ":sort",
        "Sorts the lines as written (on a page: paragraphs) of a selection or all.",
        "",
        "V to select",
        "V}:sort",
        "sort order",
    ),
    help(
        ":earlier 3  :later 3",
        "The text as it was some changes before, or after.",
        "",
        "u, gh",
        ":earlier 10",
        "undo time",
    ),
    help(
        ":registers",
        "What each register holds: what was copied, deleted, recorded.",
        "",
        "\"a to use one, p",
        "\"ayy then \"ap",
        "registers clipboard",
    ),
    help(
        ":marks",
        "The places marked with m, to go to one.",
        "",
        "ma marks a place, `a goes there",
        "ma, then later `a",
        "mark place",
    ),
    help(
        ":jumps",
        "The places jumped from, to go back to one.",
        "",
        "Ctrl-O Ctrl-I",
        ":jumps",
        "jump back",
    ),
    help(
        "g;  g,  :changes",
        "To the place of the change before, of the next; :changes lists them.",
        "",
        "u, gh",
        "g; — back to where you were writing",
        "change place last edit",
    ),
    help(
        "Ctrl-A  Ctrl-X",
        "Makes the number at or behind the cursor one larger, one smaller; with a count, that much.",
        "",
        ".",
        "5 Ctrl-A on 2020",
        "number increment",
    ),
    help(
        "Ctrl-V",
        "Selects a block of cells of a table on the page; as written, a block of columns.",
        "A block of columns of the text as written.",
        "d y on the block",
        "Ctrl-V jj in a table",
        "block column cells",
    ),
    help(
        "Shift-Tab",
        "Back to the empty place before; in a list, up.",
        "",
        "Tab",
        "Shift-Tab in a starter — the field before",
        "back tab",
    ),
    help(
        "Shift-Enter",
        "While typing on a page: a line break within the paragraph.",
        "",
        "Enter a new paragraph",
        "Shift-Enter in an address",
        "line break newline",
    ),
    help(
        "J  r Enter",
        "J joins this paragraph with the next; r and Enter on a blank parts it there.",
        "",
        "dd, o",
        "J on two halves of one thought",
        "join split paragraph",
    ),
    help(
        "r",
        "r and a character: puts it in place of the one under the cursor.",
        "",
        "x, ~ the other case",
        "r, on a comma — now a full stop",
        "replace character",
    ),
    help(
        "x",
        "In an equation, on an empty place (a missing exponent, say): takes it away with what it belongs to.",
        "",
        "Tab to reach an empty place, u",
        "x on the empty exponent of x^□",
        "equation empty place",
    ),
    help(
        "gh",
        "The history of the text: every step, with what it changed; choosing one shows the text as it was, and Enter goes back to it.",
        "",
        "u Ctrl-R, :earlier",
        "gh, then the step before the mistake",
        "history undo tree versions",
    ),
    help(
        ":w",
        "Writes the text to its file. Until then nothing is lost either: zetwerk keeps every change, and brings back what was not saved.",
        "",
        ":x saves and leaves, :q leaves, :e! goes back to what was saved",
        ":w",
        "save write",
    ),
    help(
        ":x",
        "Saves all that is open and leaves.",
        "",
        ":w, :q, ZZ",
        ":x",
        "save quit exit",
    ),
    help(
        ":q",
        "Leaves; where there are unsaved changes, says so and stays. :q! leaves without them (they are kept in the history).",
        "",
        ":x, :w",
        ":q",
        "quit exit close leave",
    ),
    help(
        ":pdf",
        "Writes the document as a PDF beside its file, under its name. Typing goes on meanwhile.",
        "",
        ":pdf 3-7 some pages, :png :svg images",
        ":pdf — paper.pdf beside paper.typ",
        "export pdf print",
    ),
    help(
        ":pdf 3-7 [file]",
        "Writes some pages as a PDF: 3-7, 5, 1,4-6, 10-, -3; with a file name, to that.",
        "",
        ":pdf",
        ":pdf 1-2 draft.pdf",
        "export pdf pages",
    ),
    help(
        ":png [pages] [300dpi] [file]",
        "Writes pages as PNG images, numbered where they are more than one.",
        "",
        ":svg, :figure png",
        ":png 1 300dpi",
        "export image png picture",
    ),
    help(
        ":svg [pages] [file]",
        "Writes pages as SVG images.",
        "",
        ":png",
        ":svg 2",
        "export image svg vector",
    ),
    help(
        ":figure png [300dpi] [file]",
        "Writes the figure the cursor is in, alone, as a PNG image.",
        "",
        ":figure svg, gF",
        ":figure png 600dpi",
        "export figure image",
    ),
    help(
        ":figure svg [file]",
        "Writes the figure the cursor is in, alone, as an SVG image.",
        "",
        ":figure png",
        ":figure svg plot.svg",
        "export figure vector",
    ),
    help(
        ":count",
        "Says how many words, characters and pages there are (without equations and code).",
        "",
        ":perf",
        ":count",
        "words count length",
    ),
    help(
        "go",
        "The headings, to go to one: type to narrow, Enter.",
        "",
        "gO to rearrange them, :labels",
        "go, then meth, then Enter",
        "outline heading goto chapter section",
    ),
    help(
        "gw",
        "Puts a label of a letter or two on every word in view; typing one goes there.",
        "",
        "/ to search, Esc",
        "gw then the letters on the word",
        "jump label hop",
    ),
    help(
        ":labels",
        "All labels of the document, to go to one.",
        "",
        "@ to refer to one, go headings",
        ":labels",
        "label goto",
    ),
    help(
        "gO",
        "The headings, to rearrange: move a section up or down with all that is under it, make it deeper or shallower. Each move is one undo step.",
        "",
        ":section up, go to go to one",
        "gO, the section, then the key for up",
        "structure outline sections reorder move chapter",
    ),
    help(
        ":section up",
        "Moves the section the cursor is in, with all under it, before the one before it.",
        "",
        "gO, :section down",
        ":section up",
        "move section",
    ),
    help(
        ":section down",
        "Moves the section the cursor is in behind the one behind it.",
        "",
        "gO, :section up",
        ":section down",
        "move section",
    ),
    help(
        ":section deeper",
        "Makes the section a level deeper, with all under it: a chapter becomes a section.",
        "",
        "gO, g+",
        ":section deeper",
        "level heading",
    ),
    help(
        ":section shallower",
        "Makes the section a level shallower, with all under it.",
        "",
        "gO, g-",
        ":section shallower",
        "level heading",
    ),
    help(
        ":references",
        "Checks the references: those to nothing, labels never referred to, labels there twice.",
        "",
        "@, gd",
        ":references",
        "check references labels",
    ),
    help(
        "gd",
        "On a reference: to what it refers to. On a name: to where it is defined.",
        "",
        "Ctrl-O back, K",
        "gd on @fig:bridge",
        "definition goto reference",
    ),
    help(
        ":<number>",
        "To that line as written; on a page, a paragraph as a rule.",
        "",
        "gg G",
        ":12",
        "line goto",
    ),
    help(
        "]]  [[",
        "To the next page, the page before.",
        "",
        "Ctrl-F, gg G, :grid",
        "]] ]] — two pages on",
        "page next previous",
    ),
    help(
        ":files",
        "The files of the document, to open one.",
        "",
        ":e, gf on a file named",
        ":files",
        "files open include",
    ),
    help(
        ":e <file>",
        "Opens a file by its name, from the document's folder.",
        "",
        ":files, :recent",
        ":e chapter2.typ",
        "open edit file",
    ),
    help(
        ":recent",
        "The documents opened last, to open one; and a new document.",
        "",
        ":new, :e",
        ":recent",
        "open recent documents back",
    ),
    help(
        ":new <name>",
        "Makes a new document in a file of its own beside this one, and offers starters for it: article, report, letter, slides, notes.",
        "",
        ":recent, Tab through the places to fill in",
        ":new essay",
        "new document create template starter",
    ),
    help(
        ":new",
        "In an empty document: the starters, to begin it with one.",
        "",
        ":new <name>",
        ":new, then article",
        "template starter",
    ),
    help(
        "]d",
        "To the next thing the compiler complains about, and what it says.",
        "",
        ":why, :errors",
        "]d",
        "error problem warning diagnostic",
    ),
    help(
        ":why",
        "Where the page stopped changing because the text cannot be typeset: says where and what, and the ways out (go there, take it out, undo to where it worked).",
        "",
        "]d, u",
        ":why, then x — what is wrong goes",
        "error why broken stopped refused problem",
    ),
    help(
        "]s  [s",
        "To the next word spelled wrong (or grammar to look at), the one before.",
        "",
        "z= what may be meant, zg it is a word",
        "]s then z=",
        "spelling spell typo",
    ),
    help(
        "z=",
        "On a word spelled wrong: what may be meant, to put in its place.",
        "",
        "]s, zg",
        "z= then Enter",
        "spelling suggest correct",
    ),
    help(
        "zg  zw",
        "zg: this is a word (it is added to your own words); zw: it is none after all.",
        "",
        "]s z=",
        "zg on a name",
        "spelling dictionary word",
    ),
    help(
        ":issues",
        "All that spelling and grammar found, to go to one.",
        "",
        "]s",
        ":issues",
        "spelling grammar list",
    ),
    help(
        ":spell",
        "Checks spelling, or stops checking.",
        "",
        ":spell ?, :grammar",
        ":spell",
        "spelling on off",
    ),
    help(
        ":spell nl",
        "Checks spelling in another language than the document's.",
        "",
        ":lang to change the document's",
        ":spell de",
        "spelling language",
    ),
    help(
        ":spell ?",
        "Says what spelling is checked in, and with which dictionaries.",
        "",
        ":spell",
        ":spell ?",
        "spelling language dictionary",
    ),
    help(
        ":grammar",
        "Checks grammar, or stops checking.",
        "",
        ":spell",
        ":grammar",
        "grammar",
    ),
    help(
        "K",
        "Says what the thing under the cursor is — an equation, a figure, a cell, a reference, a heading, a footnote — and what can be done with it.",
        "",
        "gd, :help",
        "K on a reference: what it refers to",
        "what is this describe info",
    ),
    help(
        "gs",
        "Shows the text as it is written, beside the page or by itself, and back to the page.",
        "",
        "gx an equation as written",
        "gs gs gs — round again",
        "source view split written markup code",
    ),
    help(
        ":slides",
        "Shows the pages one at a time, as slides.",
        "",
        ":grid",
        ":slides",
        "slides present presentation",
    ),
    help(
        ":grid",
        "All pages at a glance, to go to one.",
        "",
        "]] [[",
        ":grid",
        "pages overview",
    ),
    help(
        "+  -  =",
        "Larger, smaller; = as wide as the window.",
        "",
        ":ui + for the rest",
        "+ +",
        "zoom size",
    ),
    help(
        "click  drag",
        "A click puts the cursor there; held and moved, it selects.",
        "",
        "v",
        "",
        "mouse pointer",
    ),
    help(
        "double click  triple click",
        "Selects the word, the paragraph.",
        "Selects the word, the line.",
        "v V",
        "",
        "mouse pointer select",
    ),
    help(
        "Shift-click",
        "Selects from the cursor, or the selection, up to there.",
        "",
        "v",
        "",
        "mouse select",
    ),
    help(
        "click on it",
        "Takes the entry of a list that is clicked.",
        "",
        "Enter",
        "",
        "mouse list",
    ),
    help(
        "wheel  Ctrl-wheel",
        "Scrolls; with Ctrl, zooms where the pointer is.",
        "",
        "+ -",
        "",
        "mouse scroll zoom",
    ),
    help(
        ":ui +",
        "Makes the status line, the lists and the panels larger.",
        "",
        ":ui -",
        ":ui +",
        "interface size larger",
    ),
    help(
        ":ui -",
        "Makes the status line, the lists and the panels smaller.",
        "",
        ":ui +",
        ":ui -",
        "interface size smaller",
    ),
    help(
        "gD",
        "How the document looks, in one list: font, size, paper, margins, columns, language, page numbers, numbered headings, styles. Enter on one changes it; u takes it back.",
        "",
        ":styles, :font, :paper",
        "gD, then paper, then Enter",
        "document settings font paper margin layout manager",
    ),
    help(
        ":styles",
        "How headings, links, captions, tables, quotes and theorems look.",
        "",
        "gD",
        ":styles",
        "style styles look theorem",
    ),
    help(
        "gB",
        "The sources of the bibliography: list them, add one, cite one at the cursor.",
        "",
        ":bib paste, @ while typing",
        "gB, a source, Enter",
        "bibliography sources cite citation references manager",
    ),
    help(
        ":bib paste",
        "Adds the sources on the clipboard (BibTeX, RIS, CSL-JSON) to the bibliography.",
        "",
        "gB",
        ":bib paste",
        "bibliography paste import",
    ),
    help(
        ":bib import",
        "Adds the sources of another bibliography file.",
        "",
        "gB",
        ":bib import",
        "bibliography import",
    ),
    help(
        ":bib view",
        "The sources sorted, only those not cited, those there twice.",
        "",
        "gB",
        ":bib view",
        "bibliography",
    ),
    help(
        ":bib undo",
        "Takes back the last change to the bibliography file.",
        "",
        "gB",
        ":bib undo",
        "bibliography undo",
    ),
    help(
        "gF",
        "All figures and tables with caption and label: go to one, give it a label, change its width or rows.",
        "",
        ":figure, @ to refer to one",
        "gF, then the figure, Enter",
        "figures tables images manager",
    ),
    help(
        ":figure",
        "All that is said of the figure or table under the cursor.",
        "",
        "gF",
        ":figure",
        "figure table",
    ),
    help(
        ":font",
        "The fonts there are, to choose the document's.",
        "",
        "gD, :fontsize",
        ":font",
        "font typeface",
    ),
    help(
        ":fontsize 11",
        "The size of the document's text.",
        "",
        "gD, :font",
        ":fontsize 12",
        "font size",
    ),
    help(
        ":paper",
        "The papers there are, to choose the document's.",
        "",
        "gD, :margin",
        ":paper",
        "paper page a4 letter",
    ),
    help(
        ":margin 2.5cm",
        "The margins of the pages.",
        "",
        "gD, :paper",
        ":margin 3cm",
        "margin page",
    ),
    help(
        ":columns 2",
        "How many columns the text is set in.",
        "",
        "gD",
        ":columns 2",
        "columns",
    ),
    help(
        ":lang de",
        "The language of the document: of its hyphenation, quotes, and spelling.",
        "",
        "gD, :spell",
        ":lang nl",
        "language",
    ),
    help(
        ":justify",
        "Lines to both edges, or not.",
        "",
        "gD",
        ":justify",
        "justify align",
    ),
    help(
        ":spacing 0.65",
        "The space between lines.",
        "",
        "gD",
        ":spacing 1",
        "leading line spacing",
    ),
    help(
        ":indent",
        "The first line of each paragraph indented, or not.",
        "",
        "gD",
        ":indent",
        "indent paragraph",
    ),
    help(
        ":numbering",
        "Numbers before the headings, or not.",
        "",
        "gD",
        ":numbering",
        "heading numbers",
    ),
    help(
        ":pagenumbers",
        "Numbers on the pages, or not.",
        "",
        "gD",
        ":pagenumbers",
        "page numbers",
    ),
    help(
        "Tab in the last cell",
        "In the last cell of a table, Tab makes a row more and goes into it.",
        "",
        "Tab cell to cell, :table column",
        "Tab Tab Tab — through the cells, then a new row",
        "table row cell",
    ),
    help(
        ":table column",
        "A column more in the table the cursor is in.",
        "",
        ":table delete column",
        ":table column",
        "table column",
    ),
    help(
        ":table delete row",
        "Takes out the row the cursor is in.",
        "",
        "u",
        ":table delete row",
        "table row delete",
    ),
    help(
        ":table delete column",
        "Takes out the column the cursor is in.",
        "",
        "u",
        ":table delete column",
        "table column delete",
    ),
    help(
        ":agents",
        "Says whether agents (zetwerk mcp) change the text at once or propose changes for you to accept; :agents live, :agents propose.",
        "",
        "]p ga gr, gq",
        ":agents propose",
        "agent ai assistant mcp claude",
    ),
    help(
        "gn  :note",
        "A note on what is selected, or on the paragraph or figure here: kept beside the text, not in it.",
        "",
        ":notes, ]n",
        "gn, then what to look at again",
        "note comment review",
    ),
    help(
        "gq  :ask",
        "A question for an agent about what is selected, or the paragraph here.",
        "",
        "gn, :notes",
        "gq, then: is this clear?",
        "ask agent question ai",
    ),
    help(
        ":notes",
        "All notes and questions: go to one, answer, resolve, delete.",
        "",
        "gn gq ]n",
        ":notes",
        "notes review",
    ),
    help(
        "]n  [n",
        "To the next note, the one before.",
        "",
        ":notes",
        "]n",
        "notes next",
    ),
    help(
        "]p  ga  gr",
        "To the next change an agent proposes; ga accepts it, gr rejects it.",
        "",
        "zp, :agents",
        "]p, zp, ga",
        "proposal accept reject agent",
    ),
    help(
        "zp",
        "Shows the page as the proposed change would make it, and as it is.",
        "",
        "]p ga gr",
        "zp zp",
        "proposal preview",
    ),
    help(
        ":numbers",
        "Each line near the cursor says, faintly in the margin, how many lines away it is: the number for 5j or 3k.",
        "",
        "j k",
        ":numbers",
        "relative numbers lines",
    ),
    help(
        "gx",
        "Shows the equation under the cursor as it is written, and as what it means again.",
        "",
        "gs all of it as written",
        "gx in x^2 — x^2 as typed",
        "equation written source",
    ),
    help(
        ":hints",
        "A frame around what typing goes into, and words for the empty places (\"exponent\"); on or off.",
        "",
        "Tab",
        ":hints",
        "frame empty place",
    ),
    help(
        "]i  [i",
        "To the next of what the page shows as nothing — a label, a comment, a break, room — tagged in the right margin; the one before.",
        "",
        "x takes it out, Enter types it anew, :invisibles",
        "]i, then x",
        "invisible label comment hidden tag",
    ),
    help(
        "x  Enter",
        "On what shows as nothing, gone to with ]i: x takes it out, Enter types it anew.",
        "",
        "]i [i",
        "]i on a label, Enter, a new name",
        "invisible label",
    ),
    help(
        ":invisibles",
        "All that the page shows as nothing, to go to one.",
        "",
        "]i",
        ":invisibles",
        "invisible labels comments",
    ),
    help(
        ":tags",
        "The tags in the right margin for what shows as nothing, or not.",
        "",
        "]i",
        ":tags",
        "tags margin",
    ),
    help(
        ":errors",
        "The compiler's complaints beside their lines, or not.",
        "",
        "]d, :why",
        ":errors",
        "errors margin",
    ),
    help(
        ":perf",
        "What typing costs: how long a key, a compile, a frame take.",
        "",
        ":parts",
        ":perf",
        "speed performance",
    ),
    help(
        ":parts",
        "Typesets what changes by itself, at once, and all of it when typing pauses; or waits for all of it.",
        "",
        ":perf",
        ":parts",
        "speed",
    ),
    help(
        ":nextkeys",
        "After g, z, ], d and the like, the keys that can follow come up under the page if you pause; on or off.",
        "",
        "? on that list — help on those keys",
        ":nextkeys",
        "hints keys which",
    ),
    help(
        ":ghost",
        "The rest of a word or name, faint behind the cursor, for Tab to take; on or off.",
        "",
        "Tab",
        ":ghost",
        "complete completion",
    ),
    help(
        ":latex  :latex copy",
        "In an equation written in LaTeX (\\frac{a}{b}): makes it Typst's. Out of one: makes the LaTeX mathematics of the paragraph ($…$, \\[…\\]) equations. :latex copy puts the equation under the cursor on the clipboard as LaTeX.",
        "",
        ":latex on, :latex off: whether typing reads LaTeX; $, u",
        "paste a formula from a LaTeX paper, then :latex",
        "latex tex convert formula paste",
    ),
    help(
        "gH",
        "Tints the page by who wrote what: your text as it is, an agent's in a quiet colour of its own, pasted text in another; a legend says which is which. The status line tells the story of the sentence under the cursor. gH again takes the tint away.",
        "",
        ":authors, :authors age, :authors unread",
        "gH, then the cursor on a sentence an agent wrote",
        "authors authorship who wrote origin ai attribution",
    ),
    help(
        ":authors step",
        "Opens the history at the step that put the text under the cursor there.",
        "",
        "gH, gh",
        ":authors step",
        "authors history",
    ),
    help(
        ":authors",
        "How much of the document is yours, an agent's, pasted: as a whole, by section, over time.",
        "",
        "gH, :authors statement",
        ":authors",
        "authors shares",
    ),
    help(
        ":authors age",
        "Tints the page by how old its text is.",
        "",
        "gH",
        ":authors age",
        "authors age old new",
    ),
    help(
        ":authors unread",
        "Tints only what agents wrote that you have not taken in yet: not accepted, not edited since.",
        "",
        "gH, gR",
        ":authors unread",
        "authors agent unread",
    ),
    help(
        ":authors statement",
        "Writes beside the document who wrote what and which agents took part: for an \"AI use\" declaration.",
        "",
        ":authors, :authors export",
        ":authors statement",
        "authors statement declaration",
    ),
    help(
        ":authors export  :authors import",
        "Writes who wrote what beside the document as a file, for another machine or git; import reads it back.",
        "",
        ":authors statement",
        ":authors export",
        "authors export import",
    ),
    help(
        ":present  gP  F5",
        "Gives the talk: the slide the cursor is on fills the screen, nothing of the editor shows. Space goes on, Backspace back; q or Esc ends it, with the cursor on the last slide.",
        "",
        "o all slides, t the clock, p the presenter's view, :present rehearse",
        "gP on the first slide",
        "present presentation talk slides fullscreen",
    ),
    help(
        "Space  BS  12 Enter  gg  G",
        "While presenting: the next slide, the one before, slide 12, the first, the last.",
        "",
        ":present",
        "3 Enter — to the third slide",
        "present next previous slide",
    ),
    help(
        "`  Ctrl-O  b  w",
        "While presenting: back to where a jump came from; b the screen black, w white; any key brings the slide back.",
        "",
        ":present",
        "b while a question is asked",
        "present blank black",
    ),
    help(
        "o  t  p  q",
        "While presenting: all slides side by side to pick one; the clock; the presenter's view (the next slide, the notes); leave.",
        "",
        ":present",
        "o, then Enter on a slide",
        "present overview clock presenter",
    ),
    help(
        ":present rehearse",
        "Gives the talk to nobody, noting how long each slide is up.",
        "",
        ":present times, :present 20min",
        ":present rehearse",
        "present rehearse timing",
    ),
    help(
        ":present 20min",
        "Says how long the talk is to take: while presenting, the clock says what time is left and whether you are ahead or behind.",
        "",
        ":present rehearse",
        ":present 15min",
        "present time length",
    ),
    help(
        ":present times",
        "How long each slide took when the talk was rehearsed.",
        "",
        ":present rehearse",
        ":present times",
        "present timing",
    ),
    help(
        ":slide new",
        "A new slide behind the one the cursor is on.",
        "",
        ":slide up, :slides",
        ":slide new",
        "slide new",
    ),
    help(
        ":slide up  :slide down",
        "Moves the slide the cursor is on before the one before it, behind the next.",
        "",
        ":slide new, :grid",
        ":slide up",
        "slide move order",
    ),
    help(
        "H  L  n",
        "In the overview of all pages (:grid): moves the slide picked out left, right; n a new one behind it.",
        "",
        ":grid",
        ":grid, then L",
        "slide overview move",
    ),
    help(
        ":set",
        "All options in one panel, grouped by what they are about, each with its value: j and k go through them, Enter changes one, x makes it as usual, u takes the change back. What you change is kept in your settings.",
        "",
        "za the sets of aids, :set nu",
        ":set, then /night",
        "settings options preferences configure config",
    ),
    help(
        "za  :set aids=writing",
        "Goes round the sets of aids over the page: quiet (only what is wrong), writing (spelling, tags, the rest of a word), everything.",
        "",
        ":set",
        "za za",
        "aids quiet writing everything",
    ),
    help(
        ":set nu  :set nonu  :set nu!  :set nu?",
        "An option on the command line, as in Vim: on, off, switched, asked; :set nu& as usual, :set pause=300 made so.",
        "",
        ":set alone for the panel",
        ":set numbers!",
        "set option",
    ),
    help(
        "gS",
        "The pages of the document, each with what it is: its first words, a page of a PDF, a blank page. Enter on one: go there, put pages in before or behind it, move or take out a page put in.",
        "",
        ":page add, x on a page put in takes it out",
        "gS, then the cover page, Enter",
        "pages pdf insert cover appendix sheets",
    ),
    help(
        ":page add [file] [pages]",
        "Puts pages in: of a PDF (2, 1-3, all), an image as a page, a blank page; then asks where.",
        "",
        "gS",
        ":page add cover.pdf 1",
        "pages pdf insert add",
    ),
    help(
        "]r  [r  :table row add below",
        "In a table: a row below this one, above it.",
        "",
        "]c [c columns, ]x takes a row out",
        "]r in the last row",
        "table row add",
    ),
    help(
        "]c  [c  :table column add right",
        "In a table: a column right of this one, left of it.",
        "",
        "]r, ]X",
        "]c",
        "table column add",
    ),
    help(
        "]R  [R  :table row move down",
        "In a table: moves this row down, up.",
        "",
        "]C",
        "]R",
        "table row move",
    ),
    help(
        "]C  [C  :table column move right",
        "In a table: moves this column right, left.",
        "",
        "]R",
        "]C",
        "table column move",
    ),
    help(
        "]x  ]X  :table row delete",
        "In a table: takes out this row (]X this column). u brings it back.",
        "",
        "]r ]c",
        "]x on an empty row",
        "table row column delete",
    ),
    help(
        "]h  [h  :table header",
        "Makes the first row of the table its header, repeated on each page; or a row as any other again.",
        "",
        "]r",
        "]h",
        "table header",
    ),
    help(
        "]y  :table copy",
        "Copies the table as cells, for a spreadsheet. A table copied from a spreadsheet is put in as a table by p.",
        "",
        "p, \"+",
        "]y, then paste it into a spreadsheet",
        "table copy spreadsheet clipboard",
    ),
    help(
        "gA  gq  gp  gz",
        "On a suggestion of an agent: gA rewrite it yourself (its text is put in, you type in it); gq answer it in a line; gp ask for other wordings (zp goes through them); gz put it aside.",
        "",
        "ga accept, gr reject, ]p the next, gR all",
        "gp, then zp zp, then ga",
        "suggestion agent rewrite answer review",
    ),
    help(
        "gR  :review",
        "All suggestions of agents, to walk through and decide: each with why, how sure, by whom.",
        "",
        "]p ga gr, gQ, :review record",
        "gR, then Enter on one",
        "review suggestions agent ai",
    ),
    help(
        ":review accept spelling",
        "Accepts (or rejects) many suggestions at once: a category, those the agent is sure of, those here, those by one agent.",
        "",
        "gR, :review undo",
        ":review reject style",
        "review accept reject many",
    ),
    help(
        ":review undo",
        "Takes back the last decision on a suggestion (what changed the text is undone with u).",
        "",
        "gR",
        ":review undo",
        "review undo",
    ),
    help(
        "gQ",
        "Asks agents to review what is selected, or the section the cursor is in. Their suggestions come as they come.",
        "",
        "gR, :brief",
        "vip then gQ",
        "review request agent ask",
    ),
    help(
        "g!  :always",
        "Makes what you said (or what is selected) an instruction to agents for good: \"never change the quotations\".",
        "",
        ":brief lists them",
        "g! on your answer to a suggestion",
        "instruction agent always rule",
    ),
    help(
        ":brief",
        "The instructions agents are given: those that always hold, to change or take away.",
        "",
        "g!",
        ":brief",
        "brief instructions agent",
    ),
    help(
        ":review record",
        "Writes all of the review beside the document: every suggestion, what was decided, who said what.",
        "",
        "gR",
        ":review record",
        "review record export",
    ),
    help(
        "]v  [v",
        "To the next change since the file was saved (or since what :changes since names), the one before. A thin bar beside each line that changed.",
        "",
        "zc what it was, :changes list, :changes since",
        "]v ]v",
        "changes changed diff bars since",
    ),
    help(
        "zc",
        "On a change: its paragraph as it was and as it is, at once — what went struck through. Enter takes the old version back, y yanks it, Esc leaves it.",
        "",
        "]v, :revert",
        "]v then zc",
        "changes diff old version",
    ),
    help(
        ":changes list",
        "All changes since the version compared with, to go to one.",
        "",
        "]v",
        ":changes list",
        "changes list",
    ),
    help(
        ":changes since",
        "What the changes are counted from: the file as saved, as opened, a step of the history, a commit of git.",
        "",
        ":git log, ]v",
        ":changes since HEAD",
        "changes since compare",
    ),
    help(
        ":revert",
        "The paragraph the cursor is in as it was in the version compared with.",
        "",
        "zc, u",
        ":revert",
        "revert old version",
    ),
    help(
        ":changes proof",
        "The whole document marked as an editor marks a proof: new words tinted, words taken out struck through.",
        "",
        ":changes pdf",
        ":changes proof",
        "changes proof",
    ),
    help(
        ":changes pdf",
        "Writes that proof as a PDF, for someone who read the older version.",
        "",
        ":changes proof",
        ":changes pdf",
        "changes proof pdf",
    ),
    help(
        ":changes off",
        "The bars beside the lines that changed: off, or on again.",
        "",
        "]v",
        ":changes off",
        "changes bars",
    ),
    help(
        ":git log",
        "The commits of this file, to compare the text with one.",
        "",
        ":changes since, :git commit",
        ":git log",
        "git commit history version",
    ),
    help(
        ":git commit",
        "Commits the document's files — those it is made of, nothing else — after asking.",
        "",
        ":git log",
        ":git commit draft for Anna",
        "git commit",
    ),
    help(
        ":report [a few words]  g!  Ctrl-G",
        "Something felt wrong: keeps what just happened (the last keys, the text, a picture of the window) in a report on this machine, to be played again. Nothing is sent anywhere.",
        "",
        ":reports",
        "Ctrl-G right after the moment",
        "report bug wrong problem",
    ),
    help(
        ":reports",
        "The reports written, to delete them.",
        "",
        ":report",
        ":reports",
        "reports",
    ),
    help(
        "zT  :typewriter",
        "Typewriter: the line you type in stays at the same height of the window; the page moves instead.",
        "",
        "zf focus, ze zen",
        "zT, then write",
        "typewriter scroll",
    ),
    help(
        "zf  :focus",
        "Focus: all but the paragraph the cursor is in fades (:focus sentence: all but the sentence).",
        "",
        "zT, ze",
        "zf",
        "focus fade",
    ),
    help(
        "ze  :zen",
        "Zen: nothing but the text — no status line (unless it has something to say), no aids. ze again brings them back.",
        "",
        "zf, zT",
        "ze",
        "zen distraction free quiet",
    ),
    help(
        "zn  :night",
        "Night: the pages light on dark. Not what is exported or printed.",
        "",
        ":sepia, :colours",
        "zn",
        "night dark mode",
    ),
    help(
        ":sepia",
        "Sepia: the pages on warm paper.",
        "",
        "zn, :colours",
        ":sepia",
        "sepia warm paper",
    ),
    help(
        ":colours",
        "The colours of the pages: day, night, sepia, or auto (as the desktop or the hour says).",
        "",
        "zn, :sepia",
        ":colours auto",
        "colours colors dark night",
    ),
];

/// The help on the entry of the list with `keys`.
pub(super) fn help_on(keys: &str) -> Option<&'static Help> {
    HELP.iter().find(|help| help.keys == keys)
}

/// `text` broken into lines of at most [`WIDTH`] characters, each after
/// the first indented by `indent`.
fn wrapped(text: &str, indent: &str) -> String {
    let mut out = String::new();
    let mut line = 0;
    for word in text.split_whitespace() {
        let length = word.chars().count();
        if line > 0 && line + 1 + length > WIDTH {
            out.push('\n');
            out.push_str(indent);
            line = indent.chars().count();
        } else if line > 0 {
            out.push(' ');
            line += 1;
        }
        out.push_str(word);
        line += length;
    }
    out
}

/// What is said under the list of the entry that is `help`, whose
/// command is `command`.
fn told(help: &Help, command: Option<(&str, bool)>) -> String {
    let mut lines = vec![wrapped(help.page, "")];
    if !help.written.is_empty() {
        lines.push(wrapped(&format!("As written (gs): {}", help.written), "  "));
    }
    if !help.with.is_empty() {
        lines.push(wrapped(&format!("With: {}", help.with), "  "));
    }
    if !help.example.is_empty() {
        lines.push(wrapped(&format!("Example: {}", help.example), "  "));
    }
    lines.push(match command {
        Some((_, false)) => "Enter: does it".to_owned(),
        Some((_, true)) => "Enter: begins it, for you to finish".to_owned(),
        None => "Esc, then the keys".to_owned(),
    });
    lines.join("\n")
}

/// The keys of an entry one by one, as they can be asked for: what
/// stands apart by two blanks (`:section up` is one).
fn tokens(keys: &str) -> impl Iterator<Item = &str> {
    keys.split("  ")
        .map(str::trim)
        .filter(|key| !key.is_empty())
}

/// How well entry `index` (of `what`, `keys` and `help`) is about
/// `asked`: the higher, the better; `None` where it is not at all.
fn rank(asked: &str, what: &str, keys: &str, help: Option<&Help>) -> Option<u8> {
    let bare = |key: &str| key.trim_start_matches(':').to_owned();
    let asked_bare = bare(asked);
    if tokens(keys).any(|key| key == asked) {
        return Some(6);
    }
    if tokens(keys).any(|key| bare(key) == asked_bare) {
        return Some(5);
    }
    let lower = asked.to_lowercase();
    let command = |key: &str| {
        key.starts_with(':') && bare(key).split(' ').next() == Some(asked_bare.as_str())
    };
    if tokens(keys).any(|key| bare(key).to_lowercase() == asked_bare.to_lowercase() || command(key))
    {
        return Some(4);
    }
    let words = help.map_or("", |help| help.words);
    let word = |text: &str| {
        text.split(|c: char| !c.is_alphanumeric())
            .any(|word| word.to_lowercase() == lower)
    };
    if word(words) {
        return Some(3);
    }
    if word(what) {
        return Some(2);
    }
    let said = help.map_or_else(String::new, |help| {
        format!("{} {} {}", help.page, help.with, help.example).to_lowercase()
    });
    (lower.chars().count() > 2 && (said.contains(&lower) || what.to_lowercase().contains(&lower)))
        .then_some(1)
}

/// Whether help can be had with `?` behind `typed`, where what can
/// follow it is listed: not behind an operator, where `?` is a motion.
fn helpable(typed: &str) -> bool {
    matches!(
        typed,
        "g" | "z" | "Z" | "]" | "[" | "\"" | "m" | "`" | "'" | "q" | "@"
    )
}

/// The title of the list of what can follow `typed` (see `hints`).
pub(super) fn following_title(typed: &str) -> String {
    if helpable(typed) {
        format!("{typed} …        ? help on these")
    } else {
        format!("{typed} …")
    }
}

impl Viewer {
    /// `:help`, `:help <what>`, and what an entry of its list does when
    /// it is chosen. Returns whether `command` was one of them.
    pub(super) fn help_command(&mut self, command: &str) -> bool {
        let asked = match command.split_once(' ') {
            Some(("help" | "h", asked)) => asked.trim(),
            None if matches!(command, "help" | "h") => "",
            _ => return false,
        };
        if asked.is_empty() {
            self.open_help(None, 0);
            return true;
        }
        let best = palette::all()
            .enumerate()
            .filter_map(|(index, (what, keys, _))| {
                Some((rank(asked, what, keys, help_on(keys))?, index))
            })
            .max_by_key(|(rank, index)| (*rank, std::cmp::Reverse(*index)));
        if let Some((_, index)) = best {
            self.open_help(None, index);
        } else {
            self.open_help(None, 0);
            self.notice(format!(
                "nothing is about “{asked}”: type a part of it into the list"
            ));
        }
        true
    }

    /// Puts up the list of all there is to do, or of those of its
    /// entries that `only` names, with entry `chosen` chosen.
    fn open_help(&mut self, only: Option<&[usize]>, chosen: usize) {
        let mut at = 0;
        let items: Vec<(String, Target)> = palette::all()
            .enumerate()
            .filter(|(index, _)| only.is_none_or(|only| only.contains(index)))
            .map(|(index, (what, keys, command))| {
                if index == chosen {
                    at = only.map_or(index, |only| {
                        only.iter().position(|of| *of == index).unwrap_or(0)
                    });
                }
                // The keys apart: they are what is looked for, and a
                // long description is cut at its end.
                let target = match command {
                    Some((command, false)) => Target::Command(command.to_owned()),
                    Some((command, true)) => Target::Ask(command.to_owned()),
                    None => Target::Nothing,
                };
                (format!("{what}\t{keys}"), target)
            })
            .collect();
        self.open_picker("help", items, at);
    }

    /// What is said under the list of help of the entry chosen in it.
    pub(super) fn help_detail(&self) -> Option<String> {
        let picker = self
            .picker
            .as_ref()
            .filter(|picker| picker.title() == "help")?;
        let chosen = picker.chosen_label()?;
        let (_, keys, command) = palette::all()
            .find(|(what, keys, _)| chosen.split_once('\t') == Some((*what, *keys)))?;
        Some(told(help_on(keys)?, command))
    }

    /// `?` while the list of what can follow the keys typed is up: the
    /// help on those keys. Returns whether the key was taken.
    pub(super) fn help_key(&mut self, press: KeyPress) -> bool {
        if press.key != Key::Char('?') || press.ctrl || press.alt {
            return false;
        }
        let Some(typed) = self.following_listed().filter(|typed| helpable(typed)) else {
            return false;
        };
        // The key that waits waits no more.
        self.editor_key(KeyPress::plain(Key::Escape));
        let only: Vec<usize> = palette::all()
            .enumerate()
            .filter(|(_, (_, keys, _))| {
                tokens(keys).any(|key| key.len() > typed.len() && key.starts_with(typed.as_str()))
            })
            .map(|(index, _)| index)
            .collect();
        if only.is_empty() {
            self.open_help(None, 0);
        } else {
            self.open_help(Some(&only), only[0]);
        }
        true
    }

    /// `K` on something of the document: what it is, and the keys for
    /// it, as a panel. Returns whether there was something to say.
    pub(super) fn describe_thing(&mut self) -> bool {
        let Some(editor) = &self.editor else {
            return false;
        };
        let (source, cursor) = (editor.source(), editor.cursor());
        let root = LinkedNode::new(source.root());
        let Some(leaf) = root
            .leaf_at(cursor, Side::After)
            .or_else(|| root.leaf_at(cursor, Side::Before))
        else {
            return false;
        };
        let mut said = None;
        let mut node = Some(leaf);
        while let Some(here) = node {
            said = thing(&here);
            if said.is_some() {
                break;
            }
            node = here.parent().cloned();
        }
        let Some((what, keys)) = said else {
            return false;
        };
        self.tooltip = Some(format!("{what}\n{keys}"));
        true
    }
}

/// What `node` is, and the keys for it, where it is something `K` says
/// something of.
fn thing(node: &LinkedNode) -> Option<(String, &'static str)> {
    let said = match node.kind() {
        SyntaxKind::Equation => {
            let alone = node
                .children()
                .nth(1)
                .is_some_and(|inner| inner.kind() == SyntaxKind::Space);
            let what = if alone {
                "an equation, by itself"
            } else {
                "an equation, in the line"
            };
            (
                what.to_owned(),
                "i types in it · Tab: its next empty place · x on an empty place takes it · \
                 gx as written · da$ takes it all",
            )
        }
        SyntaxKind::Heading => {
            let level = node
                .children()
                .next()
                .map_or(1, |marker| marker.leaf_text().chars().count());
            (
                format!("a heading, of level {level}"),
                "g+ g- deeper, shallower · gO moves its section · go to another heading",
            )
        }
        SyntaxKind::Label => (
            format!("a label, {}: what @ refers to", node.leaf_text()),
            "]i [i go to such things · x takes it out · Enter types it anew · :labels",
        ),
        SyntaxKind::LineComment | SyntaxKind::BlockComment => (
            "a comment: the page does not show it".to_owned(),
            "]i goes to it · x takes it out · gc makes one of text",
        ),
        SyntaxKind::Strong => (
            "bold text".to_owned(),
            "di* takes what is bold · gb with a motion makes more bold · u takes it back",
        ),
        SyntaxKind::Emph => (
            "emphasised (italic) text".to_owned(),
            "di_ takes what is emphasised · gm with a motion makes more of it",
        ),
        SyntaxKind::ListItem | SyntaxKind::EnumItem => (
            "an item of a list".to_owned(),
            "o begins the next item · dd takes this one · gl makes items of paragraphs",
        ),
        SyntaxKind::Raw => (
            "code, set as written".to_owned(),
            "gt makes text code · i types in it",
        ),
        SyntaxKind::FuncCall => {
            let callee = node.children().next()?;
            let name = callee.leaf_text().as_str();
            match name {
                "figure" => (
                    "a figure".to_owned(),
                    "gF all figures · :figure all of this one · @ refers to it · \
                     Enter leaves its caption",
                ),
                "table" => (
                    "a table".to_owned(),
                    "Tab to the next cell (a row more behind the last) · :table column · \
                     :table delete row · Enter leaves a cell",
                ),
                "footnote" => (
                    "a footnote: its mark stands here, what it says at the foot of the page"
                        .to_owned(),
                    "i types in it · Enter leaves it · u takes it back",
                ),
                "cite" => (
                    "a citation of a source".to_owned(),
                    "gB the sources · @ cites another · gd goes to it",
                ),
                "image" => (
                    "an image".to_owned(),
                    "gF its figure · :figure png writes it out",
                ),
                "task" => (
                    "what to do in this step of the lesson".to_owned(),
                    "Ctrl-T the keys for it · :tutor list the lessons",
                ),
                _ => return None,
            }
        }
        _ => return None,
    };
    Some(said)
}

/// Every entry of the list of all there is to do, with its help.
#[cfg(test)]
pub(super) fn every() -> Vec<String> {
    let mut all = Vec::new();
    for (_, keys, command) in palette::all() {
        if let Some(help) = help_on(keys) {
            all.push(told(help, command));
        }
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_of_the_list_has_help_and_all_help_has_an_entry() {
        let keys: Vec<&str> = palette::all().map(|(_, keys, _)| keys).collect();
        let missing: Vec<&&str> = keys.iter().filter(|keys| help_on(keys).is_none()).collect();
        assert!(missing.is_empty(), "no help on {missing:?}");
        let orphans: Vec<&str> = HELP
            .iter()
            .map(|help| help.keys)
            .filter(|of| !keys.contains(of))
            .collect();
        assert!(
            orphans.is_empty(),
            "help on entries that are gone: {orphans:?}"
        );
        for help in HELP {
            assert_eq!(
                HELP.iter().filter(|other| other.keys == help.keys).count(),
                1,
                "help on {} twice",
                help.keys
            );
            assert!(!help.page.is_empty(), "{} says nothing", help.keys);
        }
        // None says "the source": the page is what is written on.
        for text in every() {
            let lower = text.to_lowercase();
            let source = lower.match_indices("the source").any(|(at, found)| {
                !lower[at + found.len()..].starts_with(|c: char| c.is_alphabetic())
            });
            assert!(!source, "{text}");
            assert!(
                text.lines().all(|line| line.chars().count() <= WIDTH + 2),
                "{text}"
            );
        }
    }

    #[test]
    fn help_is_found_by_keys_commands_and_words() {
        let found = |asked: &str| {
            palette::all()
                .enumerate()
                .filter_map(|(index, (what, keys, _))| {
                    Some((rank(asked, what, keys, help_on(keys))?, index, keys))
                })
                .max_by_key(|(rank, index, _)| (*rank, std::cmp::Reverse(*index)))
                .map(|(_, _, keys)| keys)
        };
        assert_eq!(found("dd"), Some("dw  dd  D"));
        assert_eq!(found("gD"), Some("gD"));
        assert_eq!(found("gd"), Some("gd"));
        assert_eq!(found(":pdf"), Some(":pdf"));
        assert_eq!(found("pdf"), Some(":pdf"));
        assert_eq!(found("footnote"), Some("#"));
        assert_eq!(found("tab"), Some("Tab"));
        assert_eq!(found("undo"), Some("u  Ctrl-R"));
        assert_eq!(found("section up"), Some(":section up"));
        assert_eq!(found("fontsize"), Some(":fontsize 11"));
        assert_eq!(found("zzyzx"), None);
    }

    #[test]
    fn lines_of_help_are_broken_between_words() {
        let text = wrapped(&"word ".repeat(40), "  ");
        assert!(text.lines().count() > 1);
        assert!(text.lines().all(|line| line.chars().count() <= WIDTH));
        assert!(text.lines().skip(1).all(|line| line.starts_with("  word")));
    }
}
