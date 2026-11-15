#set page(paper: "a4", margin: (inside: 3cm, outside: 2.5cm, y: 3cm), numbering: "1")
#set text(size: 11pt)
#set par(justify: true)
#set heading(numbering: "1.1")
#show heading.where(level: 1): it => pagebreak(weak: true) + v(2em) + it + v(1em)

#page(numbering: none)[
  #align(center + horizon)[
    #text(size: 2.2em, weight: "bold")[‹Title›]

    #v(0.5em)
    #text(size: 1.3em)[‹Subtitle›]

    #v(4em)
    #text(size: 1.2em)[‹Author›]

    #v(6em)
    ‹A thesis submitted for the degree of …› \
    ‹University, Faculty› \
    ‹Month Year›
  ]
]

#outline()

= Introduction

‹What the work is about, and why it matters.›

= Background

‹What is known already, with the sources it comes from.›

= ‹Chapter›

‹The work itself.›

= Conclusion

‹What was found, and what is left to do.›

#bibliography("refs.bib")
