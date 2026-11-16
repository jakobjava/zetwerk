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
= Search and jump labels

`/` searches for what the page reads, as in lesson 1: all that it finds
is marked, and the status line counts them; `n` goes to the next, `N` to
the one before. A search is also what many changes begin with.

The grey gulls sat on the grey wall above the grey water, and the old
skipper, who was grey himself, said it would be a grey summer.

#task[Search for the colour of everything above: `/`, the word, `Enter`.
See all of them marked.]

#task[`cgn` changes the match the cursor is on and goes on typing: `cgn`,
then "silver", then `Esc`.]

#task[`.` does the last change again, at the next match: press `.` twice.]

#task[`:%s` changes every match at once: #raw(":%s/gr" + "ey/white/g") and
`Enter`.
`u` would take all of it back in one step.]

#task[`gw` puts a label of a letter or two on every word in view. Typing
a label takes the cursor there; `Esc` takes the labels away. Press `gw`.]

/* tutor
title: Search and jump labels

step: search with /
keys: gg/grey<CR>
hint: /grey then Enter
done: cursor grey gulls

step: change the match with cgn
keys: cgnsilver<Esc>
hint: cgn, silver, Esc
done: has The silver gulls
done: mode normal

step: again with .
keys: ..
hint: . and .
done: has on the silver wall above the silver water

step: all at once with :%s
keys: :%s/grey/white/g<CR>
hint: :%s/grey/white/g then Enter
done: lacks grey
done: has white summer

step: jump labels with gw
keys: gw
hint: gw
done: jumping
*/
