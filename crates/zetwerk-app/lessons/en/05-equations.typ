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
= Equations

Mathematics is typed where it stands, and the page shows it typeset as
you go. `$` begins an equation in the line; inside it `^` raises, `_`
lowers, and letters typed together are a product: `xy` is $x y$.

#task[Type Pythagoras' theorem behind the line below: `A`, a space, `$`,
then `x^2 + y^2 = z^2`, then `Esc`.]

Pythagoras says:

#task[A fraction: behind the next line type `$`, then `ff` and `Tab`. Two
empty places come, shown as boxes; type `1`, `Tab`, `2`, then `Esc`.]

Half of it:

#task[Take the exponent of the $z$ away: put the cursor on its 2 and press
`x`. A box stays where it was: an empty place, waiting.]

#task[`Tab` goes to an empty place. Put the cursor on the $x$ of the
theorem, type `a` to begin typing behind it, press `Tab`, and type a `3`
into the box. Then `Esc`.]

#task[`#` while typing puts things in, an equation by itself among them.
Go to the last line, `o`, then `#equation` and `Enter`, then type
`E = m c^2` and `Esc`.]

The end of the lesson.

/* tutor
title: Equations

step: type x^2 + y^2 = z^2 with $
keys: /Pythagoras says<CR>A $x^2 + y^2 = z^2<Esc>
hint: A, space, $, x^2 + y^2 = z^2, Esc
done: has $x^2 + y^2 = z^2$
done: mode normal

step: a fraction with ff and Tab
keys: /Half of<CR>A $ff<Tab>1<Tab>2<Esc>
hint: A, space, $, ff, Tab, 1, Tab, 2, Esc
done: has frac(1, 2)
done: mode normal

step: take an exponent away with x
keys: /\=z^2<CR>lx
hint: put the cursor on the 2 of z², then x
done: lacks z^2
done: has = z^

step: Tab to the empty place, and fill it
keys: /\=x^2<CR>a<Tab>3<Esc>
hint: cursor on the x, a, Tab, 3, Esc
done: has = z^3$
done: mode normal

step: an equation by itself from the # list
keys: /The end of<CR>o#equation<CR>E = m c^2<Esc>
hint: o, #equation, Enter, E = m c^2, Esc
done: has E = m c^2
done: mode normal
*/
