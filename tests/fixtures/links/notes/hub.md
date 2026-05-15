# Hub

This note exercises every link shape the parser supports.

Plain wikilink: [[alpha]]
Aliased wikilink: [[beta|the beta note]]
Anchored wikilink: [[gamma#Heading One]]
Anchored + aliased: [[gamma#Heading One|G1]]
Path-form wikilink: [[sub/inner]]
Missing target (ghost): [[Phantom]]

Markdown link: [alpha](alpha.md)
Extension-less md link: [beta](beta)
URL-encoded md link: [g](sub/My%20Inner.md)
Md link to ghost: [missing](missing.md)

Embed: ![[alpha]]
Image embed: ![[diagram.png]]
Markdown embed: ![alt text](sub/inner.md)

External (not an edge): [google](https://google.com)
External mailto: [me](mailto:a@b.com)

Repeated target: [[alpha]] [[alpha]]

Inline code: see `[[alpha]]` — not a link.
Fenced code block:
```
[[alpha]]
[notalink](alpha.md)
```
Indented code block:
    [[alpha]]
    [notalink](alpha.md)
