#set page(paper: "a4", margin: (left: 3.4cm, right: 2.8cm, y: 2.6cm))
#set text(size: 11.5pt, lang: "en")
#set par(leading: 0.7em, spacing: 1.2em)
#show heading.where(level: 1): set text(size: 21pt, weight: "regular")
#show raw.where(block: false): it => box(
  fill: luma(238), inset: (x: 3pt), outset: (y: 2.5pt), radius: 2pt, it,
)
#let task(body) = block(
  above: 1.8em, below: 1em, inset: (left: 10pt, y: 4pt),
  stroke: (left: 2pt + rgb("#d79921")),
  text(fill: rgb("#3c3836"), body),
)


#text(fill: luma(120), size: 9.5pt)[zetwerk · a lesson]
= The managers

Five lists keep what a document is made of, so that nobody has to write
the rules that say it: `gD` how it looks, `gO` its sections, `gF` its
figures and tables, `gB` its sources, and `g?` all there is to do. Each is
narrowed by typing, `Enter` takes what is chosen, `Esc` goes back.

#task[`gD`: how the document looks. Give the pages numbers: `gD`, type
"page numbers", `Enter`, then choose "1" with `Enter`; `Esc` puts the list
away. `u` would take it back.]

#task[`gO`: the sections, to move. The two below are in the wrong order:
`gO`, choose "Arrival", `Enter`, then "move it up" (type `up`) and
`Enter`; then `Esc`.]

== Departure

We left the island at dawn, the nets stowed and the lanterns out.

== Arrival

The harbour was asleep when the boat came round the point.

#figure(
  image("harbour.svg", width: 50%),
  caption: [The island harbour.],
)

#task[`gF`: the figures and tables, with their captions and labels. Put it
up, then `Esc`.]

#task[`gB`: the sources of the bibliography — to add one, to paste one
from the clipboard, to cite one. Put it up, then `Esc`.]

#task[`g?`: all there is to do, each with its keys. Type a few letters of
what you want ("pdf", "spell", "zoom"); under the list it says what the
chosen one does. Put it up.]

#bibliography("refs.bib")

/* tutor
title: The managers

step: page numbers from gD
keys: gDpage numbers<CR>1<CR><Esc>
hint: gD, page numbers, Enter, 1, Enter, Esc
done: has numbering: "1"

step: a section moved up with gO
keys: <Esc>gOArrival<CR>up<CR><Esc>
hint: gO, Arrival, Enter, up, Enter, Esc
done: before == Arrival | == Departure

step: the figures with gF
keys: <Esc>gF
hint: gF
done: panel figures

step: the sources with gB
keys: <Esc>gB
hint: Esc, then gB
done: panel sources

step: all there is to do with g?
keys: <Esc>g?
hint: Esc, then g?
done: panel help
*/
