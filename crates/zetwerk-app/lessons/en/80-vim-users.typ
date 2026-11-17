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
= For Vim users: what is different on a page

Your keys work: counts, operators and text objects, registers, marks,
macros, `.`, `:s` and `:g`, `Ctrl-O`. What they work on is the page, not
the text as written, and a few of them read it as a writer would.

- A paragraph is one line to the text as written: `dd`, `yy`, `V` and `J`
  take paragraphs. `j` and `k` go by the lines the page shows, `0`, `$`, `I`
  and `A` by the line the cursor is in on the page.
- Keys take what the page shows. `x` on a reference takes all of it;
  deleting over bold text keeps its marks; nothing cuts markup in two.
- Words are what the page reads; `/` searches the page (`\=` first: the
  text as written).
- Typing `#`, `$` and `@` puts in things, equations and references;
  `Tab` takes what is offered and goes through empty places.
- `u` shows what it brought back; `gh` is the whole tree of changes.

#task[`A` types at the end of the line on the page, not of the paragraph.
In the paragraph below, on its first line, press `A`, type "!" and `Esc`:
the "!" stands in the middle of the paragraph.]

The keeper's log has an entry for every night since the light was first
lit, each in the same hand, each with the weather, the ships that passed
and the hour the lamp was trimmed, and not one night is missing from it.

#task[`dd` takes a paragraph: take the one below away with `dd`.]

This paragraph is several lines long on the page, but to the text as
written it is a single line, and `dd` takes all of it at once, as it
would take one long line in Vim.

#task[`gs` shows the text as written beside the page, `gs` again by itself,
and once more the page alone. Press `gs` once.]

#task[And `gs` twice more, back to the page.]

/* tutor
title: For Vim users

step: A types at the end of the line shown
keys: /The keeper's log<CR>A!<Esc>
hint: cursor on the first line of “The keeper's log …”, A, !, Esc
done: has !
done: lacks lamp was trimmed, and not one night is missing from it.!
done: mode normal

step: dd takes a paragraph
keys: /This paragraph is several<CR>dd
hint: cursor in it, dd
done: lacks This paragraph is several

step: the text as written with gs
keys: gs
hint: gs
done: view split

step: back to the page
keys: gsgs
hint: gs, gs
done: view page
*/
