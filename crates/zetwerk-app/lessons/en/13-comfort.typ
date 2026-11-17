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
= Comfort, and the rest

Hours at the page are easier with a few aids, each a key with `z` in front
and the same as a command; each key again turns it off. All of them, and
every other option, are in one panel, `:set`, and what you change there
is kept for the next time.

The tide turned at four, and with it the wind; by six the bay was full of
small white waves, and the boats that were still out came in one by one,
low in the water.

#task[`zT`, typewriter: the line you type in stays at one height of the
window, and the page moves instead. Press `zT`.]

#task[`zf`, focus: all but the paragraph you are in fades. Press `zf`.]

#task[`zn`, night: the pages light on dark (what is exported stays as it
is). Press `zn`, look, and `zn` again for the day.]

#task[`ze`, zen: nothing but the text — no status line, no aids. Press
`ze`, and `ze` again to come back.]

#task[`:set` lists every option, grouped, each with its value: `j` and `k`
go through them, `Enter` changes one, `x` makes it as usual again, `u`
takes a change back. `za` goes round three sets of aids: quiet, writing,
everything. Type `:set` and `Enter`.]

What else there is, each in the list of `g?` with its help: `]v` and `[v`
go to what changed since the file was saved, `zc` shows it as it was;
`gH` shows who wrote what; `gS` lists the pages and puts in pages of a
PDF; `gP` gives the talk, slide by slide; `:latex` makes LaTeX mathematics
Typst; tables have keys of their own behind `]` (`]r` a row, `]c` a
column); and when something feels wrong, `Ctrl-G` keeps a report of what
just happened, on this machine.

/* tutor
title: Comfort, and the rest

step: typewriter with zT
keys: zT
hint: zT (capital T)
done: message typewriter:

step: focus with zf
keys: zf
hint: zf
done: message focus: all but

step: night with zn, and back
keys: znzn
hint: zn, then zn again
done: message the pages as they are

step: zen with ze, and back
keys: zeze
hint: ze, then ze again
done: message zen off

step: all options with :set
keys: :set<CR>
hint: :set then Enter
done: settings
*/
