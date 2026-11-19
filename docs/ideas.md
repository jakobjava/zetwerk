# Ideas: what would make zetwerk actually good

Written down on 2026-11-13, after the page surface, the document manager and
the bibliography manager were built. The user judged all of them worth doing.
Ordered by how much each is expected to change daily use; nothing here is
started unless it says so.

## Where this stands (2026-11-16, morning): the open items, closed

Six engineers took the items listed as open below, one area each; all six
branches are merged, lint is clean and the whole suite passes.

- **Long compiles:** each compile has workers of its own; a newer text
  overtakes one that runs long (after three seconds, or four times what the
  document usually takes), at most two are left running behind, at the
  lowest priority; Ctrl-C in normal mode gives up on the one under way.
- **Names:** `#` and `@` typed on the page are the characters until
  something is chosen from their list (the file holds `\#`, `\@`); `$`
  typed again in an empty equation is a dollar sign; `A` and `I` beside an
  equation at the end or start of a line type beside it, not into it.
- **Code that shows as nothing:** a footnote or a figure goes whole or not
  at all, and the status line says what went; Ctrl-U stays in its cell;
  bracket text objects count only brackets the page shows; arrows never
  enter code; a bare link is edited letter by letter; a reference whose
  label was deleted is announced.
- **Vim:** insert mappings of several keys (`jk`, with `[keys] timeoutlen`),
  Shift-Tab, Shift-Enter, Ctrl-V (a block in the source, cells of a table on
  the page), `:s` with patterns and `c`, `:normal`, `:g`, `:sort`, `g;`,
  `@:`, Ctrl-A and Ctrl-X.
- **Files:** `:w <path>`, `:saveas`, `:mv`; the history follows a file that
  was moved or renamed outside; exports never replace a file without `!`;
  CRLF line ends are kept; the history is thinned past 10 000 states; a
  damaged journal gives back what can be read of it; a second window on the
  same file brings the first to the front and says so.
- **Managers and agents:** `gD` takes only papers and fonts that exist and
  shows what a template sets; `gB` shows entries it cannot read and keys
  used twice; the start screen lists the folder's documents; `get_state`
  tells an agent about the window, saves, open questions and notes.
- **Found by the check on GitHub:** an edit joined into a state the file had
  held before could make a document that differs from its file look saved
  (after a damaged journal, and another program writing the file). Edits
  are kept out of every such state now.

Open, as reported:

- `db` from an equation inside strong text does nothing; a few cases of the
  sweep in `viewer/tests/hidden.rs` are listed there as known.
- `/` looks for the text as typed while `:s` takes a pattern.
- Ctrl-R after an insert mapping was typed halfway may bring back its
  first key.
- A long document shows an empty window while it is typeset for the first
  time.
- Parts (merged 2026-11-16, night): page maps after a part, lines left out
  at the foot of a full page; justified text is no longer guessed while
  typing but typeset as a part, which costs more per key on a long
  document (to be measured on a quiet machine). Two cases of the random
  typist are left open and listed in `viewer/tests/typist.rs` (`KNOWN`):
  a list item begun behind typed names on the paper, and one glyph out of
  place after undo and redo on the plain text, both until the pause.
- Speed: in work (the walks from the root of the text).
- Still nothing of this was typed on a real keyboard by its authors.

Pages from PDF files, images, blank pages and page breaks (`gS`, `:pages`,
`:page add`, the agent's `insert_pages`) are merged. In work, each on a
branch of its own: suggestions from agents as a review
workflow with a record; who wrote what; a `:report`
recorder; LaTeX habits in equations; images and tables from the clipboard;
the window as it was left, typewriter scrolling, night colours; `:tutor` and
`:help`; change bars; packages; presenting; a settings panel with quiet
defaults; input methods.

## Where this stands (2026-11-15, evening): after the bug hunt

Nine engineers went through the app, one area each, using it through tests
and in a hidden window; each fix has a test that fails without it. All of it
is merged, lint is clean and the whole suite passes. In short:

- **Files:** a file changed while it was closed, or with unsaved changes
  waiting, no longer costs the text zetwerk had (`u` brings it back);
  unsaved changes brought back say so loudly, with their age, until written
  or dropped; `:q!` drops them and keeps them in the history; the word list
  and the `.bib` file are written as their owner left them.
- **Panels and agents:** an agent exports only into the document's
  directory, keeps to "propose" across a change of document, is refused a
  revision of another document, and is put into the window of its own file;
  `gD` changes the rule that holds; `g?` shows every entry's keys.
- **Frames:** the cursor, the view, the numbers of the lines, the tags and
  the notes stay where they are between a key and its pages; a compile of an
  older text takes nothing back.
- **Documents Typst refuses:** a panel says what is wrong, where, and gives
  keys out of it (go there, take it out, make it what was meant, undo, drop
  unsaved changes); `:why` shows it again; the status line says when the
  page is behind the text.
- **Typing:** Backspace and Delete join paragraphs and never reach into a
  cell, a caption, a footnote, a comment or a labelled heading; deletions
  keep the marks of strong text, the cells of a table, the name of a call.
- **Keys:** no key on the page cuts markup in two or takes what is not
  shown; `dd` and `yy` take the paragraph and `p` puts it back as one;
  words are the words the page reads; Ctrl-O/Ctrl-I, Ctrl-R, `"+`, `:s`,
  `cgn`; dead keys; what Tab does where is written at `Viewer::plain_key`.
- **Equations:** `$` types a pair; letters typed together become a product;
  what is taken out leaves an empty place or takes its mark along; an
  equation emptied goes; refused ones are all shown as written, and stay so.
- **Parts:** see below.
- **Speed** (312 pages, release, a busy machine): a key in an equation about
  820 → 18 ms, the frame after a key up to 690 → 2–12 ms, an idle frame
  161 → 0.4 ms; nothing runs when nothing happens.

Open then, as reported (see above for what became of each):

- A compile that runs very long (a huge loop) cannot be stopped; later ones
  wait behind it.
- The `#` menu still leaves `#name` or `@word` behind when left; the panel
  gets one out of it. A letter typed right behind `#sym.alpha` or a
  reference with a dot joins its name.
- Keys in code that shows as nothing: `x` on a footnote's mark or a caption,
  `d0` in a footnote, Ctrl-U in a table cell, `ci(` in a cell take parts of
  the call. `A` at a line that ends in an equation types into it.
- No Shift-Tab, no block selection, insert mappings of one key only (`jk`
  is refused).
- No `:w <path>`; a second window on the same file is refused without a
  word in the window; `:png 1 name.png` replaces a file of that name; lines
  typed into a file with CRLF line ends get LF; the history is never pruned.
- `gD`: any name is taken for the paper; a typed value that is like a
  listed one takes that one.
- Speed, each with numbers in the report of the day: wrapping the pages as a
  document after a part (15 ms on 312 pages), the walks from the root of the
  text in `typst_text` (14–30% of a Backspace there), `left_empty_everywhere`
  on every part, Typst's `Source::edit`, page maps rebuilt after every part.
- Parts: lines left out at the foot of a full page do not come back before
  the pause; the predictor breaks lines of justified text otherwise than
  Typst until then.
- Still nothing of this was typed on a real keyboard by its authors.

## Where this stood (2026-11-15, morning)

Built since, on the working tree of `page-surface` (each by an agent on a
branch of its own, merged and tested together; lint and all tests pass):

- **9, start:** `zetwerk` with no file lists recent documents and "new
  document…"; `:recent`, `:new <name>`, `:template <name>`, `:open`; starters
  (article, report, letter, slides, notes, and the user's own in
  `~/.config/zetwerk/templates`) with places to fill in that Tab goes through.
- **10, styles:** `:styles`, or the last row of `gD`: headings, title, links,
  equation font, code, quotes, captions, tables, lists, footnotes, page head
  and foot, and theorem-like environments, written as `#show`/`#let` lines
  between two marker comments and read back from them.
- **11, review:** notes and questions for agents on text (`gn`, `gq`, `]n`,
  `[n`, `:notes`), kept beside the journal, not in the file; MCP tools
  `list_notes`, `add_note`, `answer_note`, `resolve_note`; a proposal is shown
  typeset, as it would be and as it is (`zp`).
- **12, sources:** `:bib paste` (BibTeX, RIS, CSL-JSON from the clipboard),
  `:bib import`, duplicates found and merged, `:bib view` (sort, narrow, where
  cited), Hayagriva `.yml` read and added to, `:bib undo`/`redo`.
- **15, hardening:** a seeded random typist through the real viewer, property
  tests of the text functions and of the region typeset alone, a speed guard.
- **Aids over the page** (nothing of them is in the text):
  - relative line numbers in the margin (`:numbers`);
  - jump labels (`gw`);
  - a frame around what typing goes into and words for empty places
    (`:hints`);
  - tags in the right margin for what shows as nothing, `]i`/`[i` to reach
    it, and the compiler's complaints beside their line (`:tags`, `:errors`);
  - the keys that can follow, after a pause (`:nextkeys`), and the likeliest
    completion shown faintly, taken with Tab (`:ghost`);
  - search by what the page reads, with all matches marked and counted
    (`:noh`, `:hlsearch`; `\=` in front searches the text as written), and a
    flash on what was yanked or put (`:feedback`).
- **Equations:** one that Typst refuses is shown as it is written, in place,
  and `gx` shows any equation so; letters typed together that name nothing
  become a product (`xy` → `x y`); an equation that stands by itself stays in
  the middle with its number while it is typed.

Seen in a hidden window (not yet with a real keyboard): line numbers, a
refused equation and `gx`, jump labels, search marks, margin tags, the frame
and "exponent", the styles list, a new document.

Found by the random typist and not fixed (see the ignored tests in
`viewer/tests/left_behind.rs`):

- a lone `$`, a `#xyz` or a `#ref` begun and left keeps the whole document
  from being typeset.

Put right since (`viewer/tests/parts.rs`): a part read otherwise when cut
out (`*` behind `*strong*`, `- ` behind a rule) or numbered anew by Typst is
refused, not shown; references to what is not in the part show what the
whole shows (what they refer to stands hidden in the part's lead); footnotes
put in, rules among the text, columns made by a rule, floating figures, pages
wider than high, margins as shares of the page and margins of two sides no
longer make a part show other glyphs than the whole; what a part pushes below
the text of the page is left out (not drawn in the margin) until the whole
puts it on the next page; the number of an empty list item counts as part of
it, and the marks of a part's elements go in and out with it (a bullet stayed
behind, an equation broken over two lines took the page number for its own);
a part is refused where the paragraph behind it would be set otherwise. A
compile of an older text keeping what was typed since is the frame-stability
fix in `Viewer::compiled`.

Still open in parts (random typist, 25 seeds of 150 keys): where a part pushes
a line off a full page the whole may move more (a heading keeps to what
follows it), and what was left out does not come back if the part shrinks
again before the pause; `- ` typed in a list of the plain document showed a
few glyphs more than the whole once; the predictor's lines differ from the
whole in justified text (it breaks first-fit), and its room below a full page
is taken to be as wide as the margin above.

Also open: search in an equation goes by what it reads (`/a + b` does not
find `$a + b$`); Delete at the end of a paragraph does nothing (it does not
join it with the next); the key-hint pause is fixed at 600 ms.

## Where this stands (2026-11-13, evening)

Built since the list was written, all on the working tree of `page-surface`:

- **2, speed:** what changes is typeset by itself and put into the page
  (`zetwerk_typst::local`, `viewer/local.rs`; `:parts` turns it off). A key in
  an equation costs about 2 ms on the paper and 20–40 ms on the 316-page
  thesis, where all of it takes 300–900 ms. Still to do there: the 10 ms it
  takes to wrap the pages as a document again on every key (all pages are
  looked through for it), pages with columns, text in several files, and the
  list of symbols in equations (20 ms a key on the thesis).
- **3, cursor:** `f t r ~ J`, counts, empty boxes as places for the cursor, a
  cursor in an empty code block.
- **4, undo:** what `u` changed is marked; `gh` lists every step and shows it.
- **5, structure:** `gO`, `:section up|down|deeper|shallower`.
- **6, figures:** `gF`, `:figure`.
- **13, export:** `:pdf`, `:png`, `:svg` with pages, `:figure png`, off the
  window's thread.
- **14, mouse:** dragging, double and triple click, Shift-click, Ctrl-wheel.
- **7, references:** `@` lists all that can be referred to, by label, title
  or author; a thing without a label gets one when chosen; `:references`
  checks; `gd` and `K` on a reference.
- **8, spelling and grammar:** English (UK), German and Dutch from the
  system's Hunspell dictionaries (`]s [s z= zg zw`, `:spell`, `:issues`);
  grammar for English built in, for German and Dutch through a LanguageTool
  server of one's own (`[language] languagetool = "http://localhost:…"`).
  Not yet tried against a real LanguageTool server, nor with the British
  dictionary (`hunspell-en_gb` is not installed on this machine).

Not started then: 1 (needs the user at a keyboard), 9, 10, 11, 12, 15.

None of what the agents built has been seen in a real window: it is covered by
tests that drive the viewer, not by anybody's eyes.

## How it feels (2026-11-19): two rounds of ideas after a day of use

The user found the app janky at times, most of all in equations, and judged
these worth doing. Each says what was seen that led to it.

Done on 2026-11-19:

- **Equations stay equations while typed.** `frac(a)` shows as `frac(a, □)`
  in the equation being typed (it was refused and turned red); `(` and `[`
  close as they open in math. Keys in an equation were measured at 1–5 ms,
  already typeset as parts: there was no compile to wait for.
- **A display equation keeping its left edge** while typed into, centred
  again on the pause: built, and turned off by default (`:set steady`): the
  user found that it came back to the middle "very slowly and laggy".
- **A display equation stays one when typed into at its end.** What looked
  like the in-place piece standing at the left, without the number and with
  "Equation 0" behind it, was the equation become one in its line: the
  cursor on the closing `$`, behind its blank, and what was typed took the
  blank from it. What is typed there now keeps a blank behind it.
- **The cursor behind a blank in an equation** (out of an exponent, after
  `x^2 `) stood behind the 2, small, or was gone. A mark that takes no room
  is now typeset at the cursor while typing there (never in the file), and
  the cursor stands where it is, as large as the next symbol.

To do, about what is seen:

- **`/` on the page finds what the source says, too.** On the page, `/` reads
  the page: `/lambda) x` finds nothing where it shows "λ)𝑥". Where the page
  has no match, look in the source, or say that the source has one.
- **`:25` says which line it went to.** In the page view it went to line 27
  of the file. Either `:N` is always the file's line, or it says what it
  counted.
- **A key for each view.** `gs` goes round page, split, source; from the split
  back to the page is two presses. Give each view its own key or command.
- **The change bar is no second cursor.** Inside an equation, or on a slide
  whose margin is small, the bar of a changed line stands against the text
  and reads as a cursor. Keep it in the margin, or hide it on the line being
  typed.
- **The completion list in equations.** "integral" twice (`int` and the
  symbol); `inter`, `interleave`, `interrobang` above the template
  `integral_a^b dif x`; no glyph beside a symbol's name. The list also slides
  right with every letter: keep it under the start of the word.
- **Slides once while writing.** A slide with `#pause` is two pages on screen
  ("the last slides twice"); while editing, show each slide's last step only,
  as `:pdf handout` does, and all of them in `:present` and the PDF.

To do, about messages:

- **Long messages kept.** The status line now cuts what does not fit (the
  tutor's instructions lost their end). Keep recent messages for `:messages`,
  or let a key show the one standing in full.
- **Quieter while typing.** Alerts such as "no equation yet, so it is shown as
  it is written" fire mid-word; hold them back until typing pauses, and let
  notices not change the line on every key.

To do, about packages:

- **Import a package as its manual does.** `:package search` and `:package
  add` write `#import "@preview/alchemist:0.2.0"`, the module only, and a
  snippet pasted from the manual (`#skeletize(…)`) is refused. Write the
  import the package's README shows, or `: *`. (`i` in `:why` puts it right
  afterwards since 2026-11-19.)

To do, about measuring and drawing:

- **Reports that see the screen.** A Ctrl-G report says why a key waited for a
  compile, not what was on screen. Keep for each key how long until the
  screen was right, and whether a frame showed an old page or the caret
  elsewhere: the flicker of `#` typed on a slide (2026-11-19) could not be
  played again, because it is in the drawing.
- **A page swapped whole.** Never show a frame of new and old pieces of a page
  together; the guess for that flicker.
- **The frame rate on battery.** Check whether `power auto` lowers it, which
  would feel like jank whatever the compiles cost.

## 1. Real-keyboard dogfooding

Every bug the user found was one scripted tests could not see. Write two real
pages with a real keyboard and list every moment that felt wrong; fix those
before anything else.

## 2. Speed on long documents

On a thesis, anything the predictor cannot show still waits for a typing pause
and falls back to the overlay (which can wrap onto other text and shows raw
characters when a piece does not compile). Options:

- compile only the current chapter while typing, the whole document on a pause;
- extend the predictor to new paragraphs and to line reflow below an edit.

## 3. Selection and cursor polish on the page

- The cursor cannot rest on an empty box in normal mode (Tab from insert mode
  reaches it).
- `f`, `t`, `r`, `~`, `J` and counts with `h`/`l` still go by the source.
- Motions fall back to source characters while the page is behind.
- An empty code block shows no cursor.

## 4. Visual undo

Flash on the page what `u` changed, and offer a browsable history: the journal
already stores every step.

## 5. Structure panel

An outline that can be reordered: move a section up or down, promote or demote
it, fold it.

## 6. Figure and table managers

A list of all figures and tables with captions and labels: jump to one, relabel
it, resize an image with keys, set a table's alignment and rules.

## 7. Reference helper

`@` showing figures, equations, sections and sources in one list with a preview
of each target; warnings for labels that are undefined or never referred to.

## 8. Spell check and language tools

Underlines on the page, `z=` for suggestions, the language per document taken
from the document manager.

## 9. Templates and a start screen

`zetwerk` with no file: recent documents, and starters (article, thesis,
letter, slides).

## 10. Styles beyond set rules

How headings look, the colour of links, the math font and theorem environments
need `#show` rules. A "styles" page in the document manager could own those.

## 11. Review workflow with agents

Comments anchored to text, a key for "ask the agent about this selection", and
proposals shown as the page before and after, side by side.

## 12. Import of sources

Paste BibTeX from the clipboard, or have an agent resolve a DOI and call
`add_source`; detect duplicates. Manage `.yml` (Hayagriva) bibliographies too,
not only `.bib`.

## 13. Export and sharing

`:pdf` without blocking the window, export of a range of pages, a figure as PNG
or SVG.

## 14. Mouse as a helper

Clicking to place the cursor works. Dragging to select and zooming with the
wheel would cost little and take nothing from the keyboard.

## 15. Hardening (M9 of the plan)

Fuzzing of the page map and of the new edit paths, a long soak run, measurements
on battery.

## Smaller things noticed along the way

- While a name is typed after `#`, the page shows the literal `#name` until
  something is taken from the menu.
- Emptying an equation completely leaves a run of blanks between its `$` signs.
- Turning heading numbering off in a document that refers to sections makes
  Typst refuse it: the manager could warn, or say why the page stopped changing.
- Placeholder boxes for calls that lack an argument are only shown for a fixed
  list of functions outside the equation being typed.
- Agents may add sources only while they may change the text (`:agents live`).
- Changes to the bibliography file are written at once and are not undone by
  `u`.
