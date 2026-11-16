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
= Saving and exporting

zetwerk keeps every change the moment it is made: a crash or a closed
window loses nothing, and the next start brings back what was not saved.
Saving writes the text into its file, for other programs and other people.
Commands begin with `:` and end with `Enter`; `Esc` lets one go.

#task[Change something first: go to the line below, `A`, type " Saved at
last." and `Esc`. The status line shows `[+]` behind the file's name:
there are changes the file does not have.]

This is the copy of a lesson.

#task[`:w` writes the file. Type `:w` and `Enter`; the `[+]` goes.]

#task[`:count` says how long the document is: words, characters, pages.]

#task[`:pdf` writes the document as a PDF beside its file, under its name,
while you go on typing. `:pdf 2-3` would write only those pages, `:png`
and `:svg` pictures of them. Type `:pdf` and `Enter`.]

#task[`:q` leaves; with changes not saved it says so and stays. To go back
to your own documents without leaving, `:recent` lists them. Put that
list up, look, and `Esc`.]

/* tutor
title: Saving and exporting

step: a change, with A
keys: /copy of a lesson<CR>A Saved at last.<Esc>
hint: A, Saved at last., Esc
done: has Saved at last.
done: mode normal

step: write the file with :w
keys: :w<CR>
hint: :w then Enter
done: saved

step: how long it is, with :count
keys: :count<CR>
hint: :count then Enter
done: message words

step: a PDF with :pdf
keys: :pdf<CR>
hint: :pdf then Enter
done: exported pdf

step: your own documents with :recent
keys: :recent<CR>
hint: :recent then Enter
done: panel open
*/
