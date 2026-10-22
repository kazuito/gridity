#set document(title: "gridity mixed page sizes")
#set text(font: "BIZ UDGothic", size: 14pt)

#let sample(name, color, note: "↑ this edge is the top") = [
  #set align(center + horizon)
  #block(width: 100%, height: 100%, stroke: 4pt + color, inset: 24pt)[
    #text(size: 40pt, fill: color)[#name]

    #text(size: 18pt)[#note]

    #v(1em)
    #lorem(30)
  ]
]

#page(paper: "a4", margin: 1.5cm, sample("A4 portrait", rgb("#2563eb")))
#page(paper: "us-letter", margin: 1.5cm, sample("US Letter", rgb("#16a34a")))
#page(paper: "a4", flipped: true, margin: 1.5cm, sample("A4 landscape", rgb("#ea580c")))
#page(paper: "a4", margin: 1.5cm, sample("A4 + /Rotate 90", rgb("#9333ea"), note: "↑ authored top; displayed on the right"))
#page(width: 33.867cm, height: 19.05cm, margin: 1.5cm, sample("16:9 slide", rgb("#dc2626")))
