// Render with: typst compile docs/runbooks/forwarder-quick-guide.typ docs/runbooks/forwarder-quick-guide.pdf
// Prints two identical half-letter guides side by side on one US letter landscape page.

#set page(width: 11in, height: 8.5in, margin: 0in)
#set text(size: 8.5pt)
#set par(leading: 0.46em)
#set list(indent: 0.75em, body-indent: 0.32em, tight: false)
#set enum(indent: 0.75em, body-indent: 0.32em, tight: false)

#let fill-line(width) = box(width: width)[#line(length: width, stroke: 0.45pt)]
#let code(body) = box(fill: rgb("f3f3f3"), inset: (x: 2pt, y: 1pt), radius: 1.5pt, text(font: "DejaVu Sans Mono", size: 7.6pt, body))
#let note(body) = block(fill: rgb("fff5cc"), inset: 4pt, radius: 2pt)[#body]
#let warning(body) = block(fill: rgb("ffe2e2"), inset: 4pt, radius: 2pt)[#body]
#let section(title, body) = [
  #v(0.24em)
  #block(fill: rgb("eeeeee"), inset: (x: 4pt, y: 2pt), radius: 2pt)[*#title*]
  #v(0.10em)
  #body
]
#let hdr(body) = table.cell(fill: rgb("eeeeee"), inset: 3pt)[*#body*]
#let cell(body) = table.cell(inset: 3.5pt)[#body]

#let guide = [
  #align(center)[
    #text(size: 13pt, weight: "bold")[Rusty Timer Forwarder Reference]
  ]

  #v(0.22em)
  *Forwarder ID:* #fill-line(1.34in) #h(0.12in) *Ethernet IP:* #fill-line(1.34in)

  #v(0.36em)
  *Hostname UI:*
  #v(0.08em)
  #code[http://]#fill-line(2.75in)#code[.local/]

  #v(0.32em)
  *Ethernet IP UI:*
  #v(0.08em)
  #code[http://]#fill-line(2.75in)#code[/]

  #note[*Ethernet note:* The static IP works only when your computer is connected by Ethernet to the forwarder network. It will not work over Wi-Fi or hotspot.]

  #v(0.22em)
  #table(
    columns: (0.72in, 1fr, 1fr),
    stroke: rgb("d8d8d8") + 0.45pt,
    align: horizon,
    hdr[Device], hdr[Turn on], hdr[Turn off],

    cell[*Android hotspot*],
    cell[
      + Plug phone into power.
      + Hold lock/power button to turn on.
      + Unlock phone.
      + Open *Settings → Network & internet → Hotspot & tethering*.
      + Turn on *Wi-Fi hotspot*.
      + Keep phone plugged in and near forwarder.
    ],
    cell[
      + Unlock phone.
      + Swipe down from top twice.
      + Tap power icon.
      + Tap *Power off*.

      If no power icon: press *Power + Volume Up*, then tap *Power off*.

      Note: Holding only the power button may open Assistant/Gemini.
    ],

    cell[*Forwarder*],
    cell[
      + Make sure hotspot is on.
      + Connect power to forwarder.
      + Pi should turn on automatically; case fan should start.
      + If fan does not start, use small red button on side of Pi case:
        - Short press once, then press and hold until fan turns on.
      + Ready when LCD screen shows status.
      + Open UI and confirm *Ready* and reader *Connected*.
    ],
    cell[
      *Preferred:* In UI, go to *Config → Dangerous Actions* and choose *Shutdown Forwarder Device*. Wait for fan to stop.

      If the button is greyed out, enable and save *Allow restart/shutdown actions* in *Forwarder Controls*.

      *Without UI:* Use small red button on side of Pi case. Short press once, then press and hold until shutdown starts. Wait for fan to stop.
    ],
  )

  #warning[*Important:* Unplugging the forwarder does not turn it off. The forwarder has UPS backup power. Use the UI or the button to shut it down safely.]

  #section[Other operations][
    - *Readers:* Reader IPs can change. Check or update them in *Config → Readers*. After saving reader changes, restart the forwarder service if the UI says a restart is needed.
    - *Advance epoch:* In the forwarder UI, open the reader panel and use *Advance Epoch*.
  ]
]

#grid(
  columns: (5.497in, 0.006in, 5.497in),
  gutter: 0in,
  block(width: 5.497in, height: 8.5in, inset: 0.28in)[#guide],
  rect(width: 0.006in, height: 8.5in, fill: rgb("e0e0e0")),
  block(width: 5.497in, height: 8.5in, inset: 0.28in)[#guide],
)
