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

#set heading(numbering: "1.")
#show heading.where(level: 1): set heading(numbering: none)

#text(fill: luma(120), size: 9.5pt)[zetwerk · a lesson]
= When the page stops changing

Some things the page does not show: a label that names a section, a
comment, a forced break. A faint tag in the right margin stands beside
each; `]i` goes to the next of them, `x` takes it out, `Enter` types it
anew.

== The catch <catch>

The boats brought in more fish this year than in any year before.

#task[Section @catch has a label: the tag beside its heading. Go to it
with `]i` and take it out with `x`. The reference in this very line now
refers to nothing — and Typst cannot set a document like that.]

#task[The page stopped changing: it shows the text as it was before. The
status line says why, briefly; `:why` says it in full, with the ways out.
Type `:why` and `Enter`.]

#task[The panel's keys are its own while it is up: `u` goes back to the
text the page shows, `x` takes out what Typst refuses, `Enter` goes there.
Press `u`.]

That is all there is to it: when the page stops changing, `:why`.

/* tutor
title: When the page stops changing

step: take out the label with ]i and x
keys: gg]ix
hint: ]i (to the tag beside “The catch”), then x
done: lacks <catch>
done: refused

step: what is wrong, with :why
keys: :why<CR>
hint: :why then Enter
done: why

step: back to the text the page shows, with u
keys: u
hint: u, in the panel of :why
done: has <catch>
done: typeset
*/
