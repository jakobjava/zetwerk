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
= Working with an AI agent

zetwerk asks no AI anything by itself. An agent — Claude Code, say, or
any program that speaks MCP — is connected by you, with the command
`zetwerk mcp`: it reaches the window that is open, reads the text and
the pages as they are, and may change the text, look up sources, move
sections, export. Nothing of yours leaves the machine unless the agent you
connect sends it.

What it may do is yours to say. With `:agents propose` an agent only
proposes changes: each is marked on the page, `]p` goes to the next, `zp`
shows the page as it would be with it, `ga` accepts it, `gr` rejects it.
With `:agents live` its changes go in at once, each one step for `u`.

Notes are how you and an agent talk about the text. `gn` puts a note on
the paragraph the cursor is in (or on what is selected), `gq` a question
for the agent; both are kept beside the text, not in it, and `:notes`
lists them. No agent is connected now, so here you only try the keys.

#task[`:agents` says which of the two holds now. Type `:agents`, `Enter`.]

#task[A note on the paragraph below: put the cursor in it, `gn`, type
"check the date", `Enter`. A tag in the margin shows it.]

The harbour was built in 1823, or so the stone above the gate says.

#task[`:notes` lists all notes and questions, to go to one, answer it,
resolve it. Put it up.]

/* tutor
title: Working with an AI agent

step: what agents may do, with :agents
keys: :agents<CR>
hint: :agents then Enter
done: message agents

step: a note with gn
keys: /The harbour was built<CR>gncheck the date<CR>
hint: cursor in the paragraph, gn, check the date, Enter
done: message noted

step: all notes with :notes
keys: :notes<CR>
hint: :notes then Enter
done: panel notes
*/
