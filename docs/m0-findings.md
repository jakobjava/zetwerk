# M0 findings

Measured 2026-11-10 on the development laptop (18 logical cores, 1920×1200 display at scale 1.25) with Typst 0.15.1 and the embedded fonts. Reproduce with:

```sh
cargo run --release -p zetwerk-spikes -- all
```

The four spikes test the assumptions the design rests on. Where a measurement contradicted the plan, the correction is listed under [Design corrections](#design-corrections).

## 1. What a compile per keystroke would cost

One character typed into the middle of the document, then an incremental compile.

| Document | Pages | Cold compile | Per keystroke (p50) | CPU per keystroke |
|---|---|---|---|---|
| paper | 2 | 10 ms | 1.6 ms | 1.9 ms |
| article | 1 | 10 ms | 1.4 ms | 1.7 ms |
| thesis-25 | 134 | 480 ms | 130 ms | 158 ms |
| thesis-60 | 316 | 1011 ms | 310 ms | 375 ms |

- Incremental compile time grows linearly with document length, about 1 ms per page.
- It is single-threaded in practice: CPU time is 1.0–1.2× wall time whether Typst gets 1, 2, 4 or all threads. Only the cold compile uses the extra cores (thesis-25: 480 ms with all threads, 1800 ms with one).
- On a short document a compile per keystroke is cheap. On a thesis it is a full core for the whole time you type.

## 2. Rasterization (`typst-render`, one thread)

| Page | px/pt | Full page, cold | Full page, warm | Two-line band, warm |
|---|---|---|---|---|
| article | 1.4 | 8.1 ms | 1.4 ms | 0.10 ms |
| article | 3.2 | 12.9 ms | 3.7 ms | 0.17 ms |
| thesis | 1.4 | 11.7 ms | 1.9 ms | 0.14 ms |
| thesis | 3.2 | 18.1 ms | 7.1 ms | 0.31 ms |

- "Cold" means the glyph cache is empty (first time a page is shown at a zoom level); "warm" is the same content again.
- Redrawing the band around one changed line costs a fraction of a millisecond once the page has been shown. A cold band costs 2–4 ms, almost all of it computing bounding boxes for culling, which are cached afterwards.
- A region render matches the same region of a full-page render to within 1/255 per channel (glyph sub-pixel positions round differently for a different canvas origin). `crates/zetwerk-render/tests/regions.rs` holds this.
- `typst-render` is fast enough. The comparison against `vello_cpu` was not run, because nothing here calls for a second renderer.

## 3. How much of a page is editable in place

Share of glyphs by what they map to in the source.

| Document | Prose text | Spaces | Math | Not editable |
|---|---|---|---|---|
| paper | 87.0 % | 2.4 % | 7.2 % | 3.4 % |
| article | 86.6 % | 2.4 % | 9.1 % | 1.9 % |
| thesis-25 | 92.8 % | 1.9 % | 0.5 % | 4.8 % |
| slides (touying) | 88.2 % | 0.4 % | 5.6 % | 5.8 % |

- "Prose text" is verified, not assumed: a glyph only counts if the source bytes at the position Typst reports are exactly the text the glyph shows.
- The non-editable rest is almost entirely generated content without a source location: heading and equation numbers, outline entries, page numbers, footnote marks, reference text.
- A touying slide deck is as editable as a paper; only 1.1 % of its glyphs come from the package.
- Typst's `Text` syntax nodes include the single spaces between words, so typing a space inside a sentence does not change the syntax tree's shape.

## 4. Predicting a keystroke instead of compiling it

400 random single-character insertions into prose per document. The predictor (level A) reshapes the edited text item, re-justifies or shifts the rest of the line, and declines when the line would have to be re-broken.

| | paper (ragged) | article (justified) | thesis (justified) |
|---|---|---|---|
| Predicted | 80.0 % | 96.8 % | 86.0 % |
| Declined: line full | 7.2 % | 0.2 % | 5.8 % |
| Declined: alignment unknown | 12.8 % | 3.0 % | 8.2 % |
| Typst kept the line's breaks | 99.4 % | 81.9 % | 85.5 % |
| …of those, pixel-identical to the compile | 100 % | 100 % | 99.7 % |
| Cost of a prediction | 0.09 ms | 0.12 ms | 0.13 ms |
| Cost of the compile it replaces | 3 ms | 3 ms | 34 ms |

- When Typst keeps the line's breaks, the prediction is the real result: same pixels, caret error 0.000 pt. One thesis sample out of 294 was off by 1.7 pt.
- In justified text Typst re-breaks the paragraph on roughly one keystroke in six, even when the line would still fit, because its line breaker optimises the whole paragraph. Those predictions show a plausible line that changes when the compile lands. In ragged text this happens on under 1 % of keystrokes.
- "Alignment unknown" is headings, list and term items, footnotes and centred lines.
- A prediction costs about 0.1 ms regardless of document length; the compile it replaces costs 1 ms per page.

## Design corrections

1. **Predicted frames carry no source locations; the page map has to be patched, not rebuilt.** Typst assigns new span numbers to every syntax node it reparses, so the spans in a frame from before the edit do not resolve against the edited source. The predictor therefore returns the caret cell itself, and for consecutive predictions it will have to update the page map's source ranges and glyph cells directly (M4).
2. **Capping compile threads saves nothing while typing.** Incremental compiles are single-threaded. A cap only matters for the cold compile when a document is opened.
3. **`Source::find` is too slow for mapping a long single-file document.** Building the page map costs 0.15–0.45 ms per page on short documents but 8.7 ms per page on the 134-page thesis, because the lookup scans the top-level nodes linearly. M4 needs its own span lookup (binary search over siblings, or an index per source revision) and should map only visible pages.
4. **Level B (greedy re-break on overflow) would recover the 6–7 % "line full" declines, not the ~15 % re-breaks.** Those come from Typst's optimal line breaking and can only be known by compiling. Whether level B is worth building should be decided after level A is in the editor.
5. **Lines are found by proximity, not by exact abutment.** Inline math leaves sub-point gaps before the next item; a line is the chain of baseline neighbours less than 0.75 em apart.

## Not done in M0

- `vello_cpu` comparison (see spike 2).
- CI configuration for GitHub; `scripts/check.sh` runs format, lints and tests locally.
- `cargo-nextest` is not installed; tests run with `cargo test`.
