# Moving to a newer Typst

zetwerk is pinned to Typst 0.15.1 (`typst`, `typst-syntax`, `typst-layout`,
`typst-ide`, `typst-render`, `typst-svg`, `typst-pdf`, `typst-kit`, all with
`=0.15.1` in the workspace `Cargo.toml`; `hayagriva =0.10.1`, `comemo 0.5.1`
and `rustybuzz 0.20.1` are the versions that Typst uses). Typst changes what
embedders see with every minor release, and zetwerk leans on more of it than
an ordinary previewer does. This says where, so that an upgrade is a list to
work through and not a search.

## What depends on what

| What zetwerk does | Where | What of Typst it needs to stay as it is |
|---|---|---|
| Compiles, keeps fonts and files | `zetwerk-typst`: `world.rs`, `service.rs`, `lib.rs` | the `World` trait, `typst-kit` fonts, `typst::compile`, `comemo::evict`, `FileId` being minted once per path |
| Knows which glyph is which byte of the text | `zetwerk-typst/src/pagemap.rs` | `Frame`, `FrameItem::{Group, Text, Shape, Image, Link, Tag}`, `TextItem` glyphs carrying `(Span, u16)`, span numbers growing in the order of the tree (`Span::into_raw`), spans surviving `Source::edit` |
| Typesets what changed by itself and puts it into the page | `zetwerk-typst/src/local.rs`, `zetwerk-app/src/viewer/local.rs`, `typst_text::lead_for_part`, `stand_ins` | the numbering of syntax nodes being the same in a cut-down text, `Tag::Start`/`Tag::End` and `Location`s in frames, `PagedDocument::new`, how counters and `place` behave, default page margins |
| Shows a typed letter before any compile | `zetwerk-predict` | `TextItem` being constructible, the font's shaping face, `rustybuzz` shaping as Typst shapes |
| Reads the text as a tree | `zetwerk-app/src/typst_text.rs` and `typst_text/`, `invisible.rs`, `sections.rs`, `figure_text.rs`, `ref_text.rs`, `style_text.rs`, `page_text.rs`, `reading.rs` | `SyntaxKind` names, how markup, math and code nest (`LinkedNode`), error nodes and their messages (`"expected expression"`, `"unclosed delimiter"`, `"unexpected hat"` are matched by their words in `typst_text/shown.rs`, `typst_text.rs` and `viewer/refused.rs`) |
| Says what is wrong in a writer's words | `viewer/refused.rs`, `viewer/margin.rs` | the wording of Typst's diagnostics (matched by their beginnings) |
| Completes, explains, jumps | `viewer/assist.rs`, `viewer/refer.rs` | `typst_ide::{autocomplete, tooltip, definition}`, the introspector's queries |
| Draws pages | `zetwerk-render` | `typst_render::render`, items being comparable for the tile cache |
| Exports | `viewer/export.rs` | `typst_pdf::pdf`, `typst_svg`, PDF images (`image(.., page:)`) |
| Writes rules for the user | `viewer/manager.rs`, `style_text.rs`, the starters in `zetwerk-app/templates/` | the names and arguments of `page`, `text`, `par`, `heading`, `figure`, `math.equation`, … as written into documents |

## How to do it

1. Read Typst's changelog for everything between the two versions, with the
   table above beside it.
2. On a branch: change the pinned versions (all of them at once; `hayagriva`,
   `comemo` and `rustybuzz` to what the new Typst uses), `cargo update -p typst`
   and its family, and get it to compile. `zetwerk-typst`, `zetwerk-predict`
   and `zetwerk-render` are where the compiler will complain; `zetwerk-app`
   mostly where `SyntaxKind`s were renamed.
3. Run `./scripts/check.sh`. The tests that say most about Typst having moved:
   - `zetwerk-typst`: the page map's round trip over the corpus, and
     `tests/local_region.rs` (`the_cut_keeps_the_numbers_of_the_region`);
   - `zetwerk-app`: `viewer/tests/parts.rs` and the random typist
     (`viewer/tests/typist.rs`), which compare a part typeset by itself with
     the whole; `viewer/tests/errors.rs` (the wording of diagnostics);
     `style_text` and `viewer::start` (every rule and every starter compiles);
   - `zetwerk-predict`: the accuracy suite against true compiles.
4. Then by eye, in a hidden window (see `/home/jakjava/zetwerk-agents/tools/README.md`
   on this machine): type in prose and in an equation on the corpus paper and
   slides, and compare with a picture from before the upgrade.
5. Measure: `ZETWERK_SPEED=350 cargo test --release --workspace
   what_keys_and_frames_cost -- --ignored --nocapture` before and after. A new
   Typst may be faster or slower at exactly the things done on every key.

## What would make it easier

- The words of diagnostics are matched in three places; one table of them
  (message → meaning) would be one place to look.
- `local.rs` assumes how Typst numbers nodes; that is the most likely thing to
  break silently. Its strict test is the guard: never ignore it.
- When Typst gets partial compiles or cancellation of its own, the part
  typeset by itself and the abandoned compile threads can go.
