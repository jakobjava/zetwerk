#set page(paper: "a4", margin: (x: 2.5cm, y: 3cm), numbering: "1")
#set text(size: 11pt)
#set par(justify: true)
#set heading(numbering: "1.1")

#align(center)[
  #text(size: 1.8em, weight: "bold")[‹Title›]

  #v(0.8em)
  #grid(
    columns: 2,
    column-gutter: 3em,
    [‹Author› \ #text(size: 0.9em)[‹Affiliation›]],
    [‹Author› \ #text(size: 0.9em)[‹Affiliation›]],
  )
]

#v(1.5em)
#pad(x: 3em)[
  #align(center)[*Abstract*]

  ‹What the article is about, what was done and what was found, in a few sentences.›
]
#v(1.5em)

= Introduction

‹Why the question matters, and what this article adds to what is known.›

= Method

‹How the work was done.›

= Results

‹What was found.›

= Conclusion

‹What it means, and what is left to do.›
