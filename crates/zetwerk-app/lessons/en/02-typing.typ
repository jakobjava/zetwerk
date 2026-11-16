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
= Typing and deleting

Keys do one of two things in zetwerk: while you type, they put letters on
the page; otherwise they are commands, as in this lesson. `i` begins typing,
`Esc` ends it. The status line says which it is: NORMAL or INSERT.

#task[`dw` deletes from the cursor to the next word. The sentence below
has one word twice: put the cursor on the second of the two (`/` gets you
near, `w` goes on a word) and press `dw`.]

This sentence has a word word too many.

#task[`x` deletes the character under the cursor. One of the words below
has a letter three times where it should have it twice: take one away.]

A misspellling is easy to make.

#task[`i` types before the cursor, `Esc` stops typing. The cat below has
nothing to sit on: put "the" before "mat", with a space behind it.]

The cat sat on mat.

#task[`o` begins a new paragraph below the cursor's, and types in it. Go
to the line below, press `o`, type "Ships need harbours." and press
`Esc`.]

Write your sentence under this one.

#task[`dd` takes a whole paragraph away. Take away the paragraph that asks
for it.]

Please take this whole paragraph away: it is not needed here.

#task[`cw` changes a word: it deletes it and you type the new one. Give
the sky below another colour: `cw` on the colour, the new colour, `Esc`.]

The sky over the bay is blue today.

That is typing and deleting. Lesson 3 is about taking things back.

/* tutor
title: Typing and deleting

step: take out the word that is there twice
keys: /has a<CR>wwwdw
hint: /has a then Enter, w w w, then dw
done: lacks word word
done: has has a word too many

step: take out the letter there three times
keys: /misspel<CR>llllllx
hint: put the cursor on one of the three l's (l moves right), then x
done: has A misspelling is easy

step: put “the” before “mat”
keys: /sat on<CR>wwithe <Esc>
hint: /sat on, Enter, w w (to “mat”), then i, the, space, Esc
done: has sat on the mat
done: mode normal

step: a new paragraph with o
keys: /Write your<CR>oShips need harbours.<Esc>
hint: go to “Write your sentence …”, then o, Ships need harbours., Esc
done: has Ships need harbours.
done: mode normal

step: take a paragraph away with dd
keys: /Please take<CR>dd
hint: put the cursor in it, then dd
done: lacks Please take this whole paragraph

step: change a word with cw
keys: /blue<CR>cwgreen<Esc>
hint: /blue, Enter, cw, a colour, Esc
done: lacks is blue today
done: has The sky over the bay is
done: mode normal
*/
