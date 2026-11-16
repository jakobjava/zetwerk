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
= Moving on the page

The cursor is the dark block on the title. Everything in zetwerk is done
with keys, and these move it. This page is a copy made for you: nothing you
do here touches your own documents. The status line says which step you are
at; a tick in the margin says when one is done; `Ctrl-G` gives the keys
when you are stuck.

#task[`j` goes down a line, `k` goes up one: the lines as the page shows
them, even inside a paragraph. Press `j` until the cursor is in the
paragraph about the harbour, below.]

The old harbour lies at the end of the bay, behind a wall of grey stones.
Boats come in at dusk and leave before the sun is up, and the gulls know
the hour better than the fishermen do.

#task[The quickest way anywhere: `/`, a word, then `Enter` goes to where
the page next reads it, and `n` on to the place after that. Type
`/lantern` and `Enter` — it finds this very line first — then `n` to go
on to the paragraph below.]

At night the keeper lights the lantern in the tower and climbs down the
hundred steps again, counting them as he goes.

#task[`w` goes to the next word, `b` back one, `e` to the end of a word.
From "lantern", press `w` until you are on "tower".]

#task[`$` goes to the end of the line, `0` to its start. Go to the line
below and press `$`.]

A short line, to go to the end of.

#task[`G` goes to the end of the document, `gg` back to its beginning.
Press `G`.]

That is the end: the last line of the page. `gg` takes you back up.

/* tutor
title: Moving on the page

step: down with j, into the paragraph about the harbour
keys: jjjjjjjjjjjj
hint: j, j, j … until the cursor is in “The old harbour …”
done: below The old harbour

step: to the word “lantern” with /
keys: /lantern<CR>n
hint: /lantern then Enter, then n
done: cursor lantern in the tower

step: on to “tower” with w
keys: www
hint: w, w, w
done: cursor tower

step: to the end of the short line with $
keys: /short line<CR>$
hint: go to “A short line” (j or /short), then $
done: cursor of.

step: to the end of the document with G
keys: G
hint: G (capital: Shift-g)
done: below That is the end
*/
