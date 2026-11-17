#set page(paper: "a4", margin: (left: 3.4cm, right: 2.8cm, y: 2.6cm))
#set text(size: 11.5pt, lang: "en")
#set par(leading: 0.7em, spacing: 1.2em)
#show heading.where(level: 1): set text(size: 21pt, weight: "regular")
#show raw.where(block: false): it => box(
  fill: luma(238), inset: (x: 3pt), outset: (y: 2.5pt), radius: 2pt, it,
)
#let task(body) = block(
  above: 1.8em, below: 1em, inset: (left: 17pt, y: 4pt),
  stroke: (left: 2pt + rgb("#d79921")),
  text(fill: rgb("#3c3836"), body),
)


#text(fill: luma(120), size: 9.5pt)[zetwerk · a lesson]
= Footnotes, tables, figures

What a paper needs besides words is put in while typing: `#` and its name.
The list narrows as you type; `Enter` takes what is chosen. Whatever you
put in, `Enter` at its end takes you out again, back into the sentence.

#task[A footnote: at the end of the line below type `A`, then `#foot` and
`Enter`. Type "From the old charts." and press `Esc`.]

The bay is eleven fathoms deep at the harbour mouth

#task[A table: go to the line below, `o`, then `#table` and `Enter`. Type
into its cells, `Tab` going from one to the next: Port, Depth, Dover, 12.
Then `Esc`.]

The depths of a few ports:

#task[A figure: on the line below, `o`, then `#figure` and `Enter`. A list
of the images beside the document comes up: type `harb` and `Enter` for
the harbour. Then `Esc`.]

What the harbour looks like:

#task[`gF` lists all figures and tables of the document, with caption and
label: put it up, look, and `Esc`.]

/* tutor
title: Footnotes, tables, figures

step: a footnote with #foot
keys: /eleven fathoms<CR>A#foot<CR>From the old charts.<Esc>
hint: A, #foot, Enter, From the old charts., Esc
done: has #footnote[From the old charts.]
done: mode normal

step: a table with #table, and Tab between cells
keys: /depths of a few<CR>o#table<CR>Port<Tab>Depth<Tab>Dover<Tab>12<Esc>
hint: o, #table, Enter, Port, Tab, Depth, Tab, Dover, Tab, 12, Esc
done: has [Dover]
done: has [12]
done: mode normal

step: a figure with #fig
keys: /harbour looks<CR>o#figure<CR>harb<CR><Esc>
hint: o, #figure, Enter, harb, Enter, Esc
done: has image("harbour.svg")
done: mode normal

step: all figures and tables with gF
keys: gF
hint: gF (capital F)
done: panel figures
*/
