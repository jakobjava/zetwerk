# Ideas: what would make zetwerk actually good

Written down on 2026-11-13, after the page surface, the document manager and
the bibliography manager were built. The user judged all of them worth doing.
Ordered by how much each is expected to change daily use; nothing here is
started unless it says so.

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

Not started: 1 (needs the user at a keyboard), 9, 10, 11, 12, 15.

None of what the agents built has been seen in a real window: it is covered by
tests that drive the viewer, not by anybody's eyes.

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
