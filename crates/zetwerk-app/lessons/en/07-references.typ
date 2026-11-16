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
= References and citations

A reference says "see Figure 1" and keeps saying the right number,
whatever moves. While typing, `@` lists everything that can be referred
to — figures, tables, equations, numbered sections, sources — by label,
by caption, by author; `Enter` puts the reference in.

#figure(
  image("harbour.svg", width: 60%),
  caption: [The harbour at low tide.],
)

#task[Refer to the figure above: at the end of the line below, `A`, a
space, then `@`, a few letters of its caption ("low tide" will do),
`Enter`, and `Esc`. A figure without a label gets one.]

The lighthouse stands on the right, as shown in

#task[`K` on a reference says what it refers to. Put the cursor on the
reference you just made and press `K`; `Esc` puts it away.]

#task[`gd` goes to what a reference refers to, and `Ctrl-O` back again.
On the reference, press `gd`.]

#task[Sources are cited the same way: `@` lists them too. At the end of the
line below type `A`, a space, `@`, "knuth", `Enter`, `Esc`. The
bibliography at the end of the document gets its entry.]

Writing a program is a kind of literature

#bibliography("refs.bib")

/* tutor
title: References and citations

step: refer to the figure with @
keys: /as shown in<CR>A @low tide<CR><Esc>
hint: A, space, @, low tide, Enter, Esc
done: has as shown in @
done: mode normal

step: K on the reference
keys: /as shown in<CR>$K
hint: put the cursor on the reference ($ goes to the end of the line), then K
done: message igure

step: gd to the figure
keys: <Esc>gd
hint: on the reference: gd
done: above The lighthouse stands

step: cite a source with @
keys: /a kind of literature<CR>A @knuth<CR><Esc>
hint: A, space, @, knuth, Enter, Esc
done: has @knuth1984
done: mode normal
*/
