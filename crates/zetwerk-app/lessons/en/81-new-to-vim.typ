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
= Never used Vim? The ten keys that matter

zetwerk is worked with keys alone, after the manner of Vim, an old and
much loved editor: keys are commands until you begin to type. That sounds
strange for a day, and then the hands keep to the keyboard and the eyes
to the page. These ten are enough for a whole paper; `g?` lists the rest.

#table(
  columns: 2,
  stroke: none,
  inset: (x: 0pt, y: 4pt),
  column-gutter: 14pt,
  [`i`], [type, before the cursor (`a` behind it, `o` in a new paragraph)],
  [`Esc`], [stop typing: keys are commands again],
  [`h` `j` `k` `l`], [left, down, up, right — or the arrow keys],
  [`/`], [go to a word: `/`, the word, `Enter`; `n` the next],
  [`x`], [delete the character under the cursor],
  [`dd`], [delete the paragraph (`dw` a word)],
  [`u`], [undo, as often as you like (`Ctrl-R` redo)],
  [`:w`], [save (`:q` leave)],
  [`g?`], [all there is to do, each with its keys],
  [`Ctrl-T`], [in a lesson: the keys of the step at hand],
)

#task[Type a sentence of your own into the empty line below: go to the
line ("/Your" finds it), `o`, the sentence ending in a full stop, then
`Esc`.]

Your sentence goes under this line.

#task[Take one letter of it away with `x`, then bring it back with `u`.]

#task[Save with `:w` and `Enter`.]

#task[`g?` and the word "pdf": what the list says about writing a PDF.
That is the end of the course; `:tutor list` shows all lessons again.]

/* tutor
title: Never used Vim? The ten keys

step: type a sentence with o
keys: /Your sentence<CR>oThe sea was calm.<Esc>
hint: /Your, Enter, o, a sentence, Esc
done: changed
done: has Your sentence goes under this line.
done: mode normal

step: x on a letter
keys: x
hint: x on a letter of your sentence
done: changed
done: mode normal

step: undo it with u
keys: u
hint: u
done: back

step: save with :w
keys: :w<CR>
hint: :w then Enter
done: saved

step: help with g?
keys: g?pdf
hint: g?, then type pdf
done: panel help
*/
