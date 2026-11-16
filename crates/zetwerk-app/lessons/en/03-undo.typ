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
= Undo

Nothing is lost in zetwerk. Every change is kept, saved or not, even
after the window is closed, and `u` takes changes back one by one, as far
back as you like: the page marks what came back.

#task[Delete the paragraph below with `dd`. It will come back.]

The lighthouse was built in the year the harbour froze over.

#task[`u` undoes: press it, and the paragraph is back.]

#task[`Ctrl-R` does again what `u` undid: the paragraph goes once more.]

#task[`gh` puts up the history: every step of the text, newest first.
Choosing a step shows the text as it was then; `Esc` puts the list away.]

#task[Now undo all of it: `Esc`, then `u` until the page is as it came.
When there is nothing left to undo, `u` says so.]

The keeper never missed a night in forty years.

/* tutor
title: Undo

step: delete a paragraph with dd
keys: /The lighthouse was<CR>dd
hint: put the cursor in “The lighthouse was built …”, then dd
done: lacks The lighthouse was built

step: undo with u
keys: u
hint: u
done: has The lighthouse was built

step: redo with Ctrl-R
keys: <C-r>
hint: Ctrl-R (hold Ctrl, press r)
done: lacks The lighthouse was built

step: the history with gh
keys: gh
hint: gh
done: panel history

step: undo all of it
keys: <Esc>uuu
hint: Esc, then u, u, u …
done: unchanged
*/
