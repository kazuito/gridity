#set document(title: "gridity sample report")
#set page(paper: "a4", margin: 2cm, footer: context align(center)[#counter(page).display("1 / 1", both: true)])
#set text(font: "BIZ UDGothic", size: 11pt, lang: "ja")
#set heading(numbering: "1.")

#let palette = (rgb("#2563eb"), rgb("#16a34a"), rgb("#ea580c"), rgb("#9333ea"), rgb("#dc2626"), rgb("#0891b2"))

#let bars(n) = {
  let values = range(8).map(i => calc.rem(i * 37 + n * 11, 90) + 10)
  box(width: 100%, height: 6cm, stroke: 0.5pt + gray, inset: 8pt)[
    #grid(
      columns: (1fr,) * values.len(),
      column-gutter: 6pt,
      align: bottom,
      ..values.map(v => rect(width: 100%, height: v * 1% * 5cm, fill: palette.at(calc.rem(n, palette.len()))))
    )
  ]
}

#for n in range(1, 13) [
  = 第 #n 章: Section #n

  #lorem(60)

  この段落は日本語の表示確認用です。グリッドに並べたときに、文字が読めるかどうかを確認してください。ページ番号は下部に表示されます。

  #figure(bars(n), caption: [sample bars])

  #table(
    columns: (auto, 1fr, 1fr, 1fr),
    fill: (_, y) => if y == 0 { palette.at(calc.rem(n, palette.len())).lighten(70%) },
    [*Item*], [*Q1*], [*Q2*], [*Q3*],
    ..range(4).map(r => ([Row #(r + 1)], [#(n * 10 + r)], [#(n * 7 + r * 3)], [#(n * 5 + r * 9)])).flatten()
  )

  #lorem(40)
  #if n < 12 { pagebreak(weak: true) }
]
