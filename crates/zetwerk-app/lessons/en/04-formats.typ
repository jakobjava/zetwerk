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
= Bold, italic, headings, lists

A key with `g` in front makes text look different: `gb` bold, `gm`
italic, `g+` a heading, `gl` a list. Behind it comes what it is to cover,
as behind `d`: `iw` the word under the cursor, `ip` the paragraph.

#task[Make the word "strong" below bold: put the cursor on it, then `gb`
and `iw`.]

Only one word in this line is strong.

#task[Now make "softly" italic: `gm` and `iw` on it.]

The tide came in softly that night.

#task[While typing, `Ctrl-B` begins bold and ends it again (`Ctrl-I` the
same for italic). Go to the end of the line below (`A`), type a space,
`Ctrl-B`, "loud", `Ctrl-B`, a full stop and `Esc`.]

The foghorn was

#task[`g+` makes a heading of what it covers: `g+` and `ip` on the line
below.]

Life on the island

#task[`gl` makes items of a list: select the three paragraphs below with
`V` and `j j`, then press `gl`.]

nets

ropes

lanterns

That is how text gets its looks. The document as a whole — its font, its
paper — is the business of `gD`, in lesson 9.

/* tutor
title: Bold, italic, headings, lists

step: a word bold with gb iw
keys: /is strong<CR>wgbiw
hint: /is strong, Enter, w (to the word), then gb, then iw
done: has is *strong*

step: a word italic with gm iw
keys: /in softly<CR>wgmiw
hint: /in softly, Enter, w, then gm, then iw
done: has came in _softly_

step: bold while typing with Ctrl-B
keys: /The foghorn<CR>A <C-b>loud<C-b>.<Esc>
hint: A, a space, Ctrl-B, loud, Ctrl-B, ., Esc
done: has *loud*
done: mode normal

step: a heading with g+ ip
keys: /Life on<CR>g+ip
hint: put the cursor on “Life on the island”, then g+ and ip
done: has = Life on the island

step: a list with V and gl
keys: /nets<CR>Vjjgl
hint: cursor on “nets”, V, j, j, then gl
done: has - nets
done: has - lanterns
*/
