#!/usr/bin/env python3
"""brnr's diagram: the editor, brnr acp, the brnr process and the agent, and
what reaches the process besides. One description, drawn in the site's two
schemes, writes next to this file and in docs/brand/, for the book (whose
local build has no site/ beside it):

  diagram-light.svg  the light scheme of site/index.html
  diagram-dark.svg   its dark scheme

Each has the page's colour as its background, so it reads the same on any
page behind it (GitHub's white or dark, crates.io, the book, the site). Plain
SVG: text in system fonts, no web fonts, scripts or external references,
which GitHub and crates.io strip or block.
"""

import os
from xml.sax.saxutils import escape

HERE = os.path.dirname(os.path.abspath(__file__))
BOOK = os.path.join(HERE, "..", "..", "docs", "brand")
W, H = 960, 476
SANS = "ui-sans-serif, -apple-system, 'Segoe UI', sans-serif"
MONO = "ui-monospace, 'SF Mono', Menlo, monospace"

SCHEMES = {  # site/index.html's :root, and its prefers-color-scheme: dark block
    "light": dict(
        page="#e4e7df", ink="#24291d", muted="#59604d", rule="#c3c8b7", box="#d6dbcc",
        line="#59604d", bezel="#2a2e27", lcd="#b8c795", lcd_ink="#26331a",
    ),
    "dark": dict(
        page="#1c1f1a", ink="#e1e6d6", muted="#9da48f", rule="#383d32", box="#262a22",
        line="#9da48f", bezel="#0f110d", lcd="#9fd06a", lcd_ink="#17260c",
    ),
}  # fmt: skip

TITLE = "brnr's architecture"
DESC = (
    "An optional editor's session: the editor runs brnr acp over stdio, and brnr acp, "
    "which only relays bytes, is joined to the brnr process by socketpairs. The brnr "
    "process owns the agent (Claude Code, Codex, any ACP agent) and talks to it over "
    "pipes. A headless session has no editor and no brnr acp. Below the brnr process is "
    "its control socket, a Unix socket only you can open. You reach it from a terminal "
    "(brnr prompt send, brnr event watch, and the other commands), and another agent "
    "reaches it through the same commands and brnr's skill. Bridges, such as Slack or "
    "push notifications, are started by a profile and talk to the brnr process over pipes."
)

# Boxes: (x, y, w, h, title, lines, mono, kind); kind picks the colours.
ROW1, ROW1_H = 84, 88
ROW3, ROW3_H = 372, 84
# fmt: off
EDITOR = (28, ROW1, 100, ROW1_H, "editor", [], False, "box")
ACP = (184, ROW1, 132, ROW1_H, "brnr acp", ["relays bytes"], False, "box")
PROCESS = (452, ROW1, 240, ROW1_H, "brnr process", ["owns the agent"], False, "lcd")
AGENT = (764, ROW1, 172, ROW1_H, "agent", ["Claude Code, Codex,", "any ACP agent"], False, "box")
SOCKET = (380, 240, 280, 60, "control socket", ["a Unix socket only you can open"], False, "box")
YOU = (28, ROW3, 280, ROW3_H, "you, in a terminal", ["brnr prompt send", "brnr event watch, …"], True, "box")
OTHER = (380, ROW3, 280, ROW3_H, "another agent", ["the same commands,", "with brnr's skill"], False, "box")
BRIDGES = (732, ROW3, 204, ROW3_H, "bridges", ["Slack, push, …", "a profile starts them"], False, "box")
BOXES = (EDITOR, ACP, PROCESS, AGENT, SOCKET, YOU, OTHER, BRIDGES)
# fmt: on

SOCKET_X = SOCKET[0] + SOCKET[2] // 2  # the socket's line leaves the process here...
BRIDGE_X = 684  # ...and the bridges' here, right of the socket
BUS_Y = 336  # where the socket's line splits for its two clients
MID1 = ROW1 + ROW1_H // 2


def text(x, y, s, c, size, weight=400, family=SANS, anchor="middle", style=""):
    st = f' font-style="{style}"' if style else ""
    return (
        f'<text x="{x}" y="{y}" fill="{c}" font-family="{family}" font-size="{size}" '
        f'font-weight="{weight}" text-anchor="{anchor}"{st}>{escape(s)}</text>'
    )


def box(b, p):
    x, y, w, h, title, lines, mono, kind = b
    fill, stroke, ink, sub = (
        (p["lcd"], p["bezel"], p["lcd_ink"], p["lcd_ink"])
        if kind == "lcd"
        else (p["box"], p["rule"], p["ink"], p["muted"])
    )
    cx = x + w // 2
    n = len(lines)
    top = y + h // 2 - (n * 20) // 2 + 7  # the title's baseline, the block centred
    out = [
        f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="10" fill="{fill}" '
        f'stroke="{stroke}" stroke-width="1.5"/>',
        text(cx, top, title, ink, 20, 700),
    ]
    for i, line in enumerate(lines):
        out.append(text(cx, top + 23 + i * 20, line, sub, 16, family=MONO if mono else SANS))
    return out


def path(d, p, dash=""):
    da = f' stroke-dasharray="{dash}"' if dash else ""
    return f'<path d="{d}" fill="none" stroke="{p["line"]}" stroke-width="1.5"{da}/>'


def link(x0, x1, label, p):
    """A link on the main line, its transport above it."""
    return [path(f"M{x0} {MID1}H{x1}", p), text((x0 + x1) // 2, MID1 - 9, label, p["muted"], 16)]


def svg(p):
    right = lambda b: b[0] + b[2]  # noqa: E731
    bottom = lambda b: b[1] + b[3]  # noqa: E731
    centre = lambda b: b[0] + b[2] // 2  # noqa: E731
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" '
        f'height="{H}" role="img" aria-labelledby="title desc">',
        f'<title id="title">{escape(TITLE)}</title>',
        f'<desc id="desc">{escape(DESC)}</desc>',
        f'<rect width="{W}" height="{H}" rx="12" fill="{p["page"]}"/>',
        # The editor's part, which a headless session doesn't have.
        f'<rect x="12" y="24" width="320" height="164" rx="14" fill="none" '
        f'stroke="{p["muted"]}" stroke-width="1.5" stroke-dasharray="6 5"/>',
        text(
            28, 56, "optional: an editor's session", p["muted"], 15, anchor="start", style="italic"
        ),
        *link(right(EDITOR), ACP[0], "stdio", p),
        *link(right(ACP), PROCESS[0], "socketpairs", p),
        *link(right(PROCESS), AGENT[0], "pipes", p),
        # The control socket, below the process, and its clients.
        path(f"M{SOCKET_X} {bottom(PROCESS)}V{SOCKET[1]}", p),
        path(f"M{SOCKET_X} {bottom(SOCKET)}V{ROW3}", p),
        path(f"M{SOCKET_X} {BUS_Y}H{centre(YOU)}V{ROW3}", p),
        # Bridges: the process's children, on pipes of their own.
        path(f"M{BRIDGE_X} {bottom(PROCESS)}V{BUS_Y}H{centre(BRIDGES)}V{ROW3}", p),
        text(BRIDGE_X + 10, 262, "pipes", p["muted"], 16, anchor="start"),
        text(28, 236, "A headless session has no editor", p["muted"], 16, anchor="start"),
        text(28, 256, "and no brnr acp.", p["muted"], 16, anchor="start"),
    ]
    for b in BOXES:
        out += box(b, p)
    out.append("</svg>\n")
    return "\n".join(out)


os.makedirs(BOOK, exist_ok=True)
for name, scheme in SCHEMES.items():
    for folder in (HERE, BOOK):
        with open(os.path.join(folder, f"diagram-{name}.svg"), "w") as f:
            f.write(svg(scheme))
