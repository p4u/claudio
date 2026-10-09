#!/usr/bin/env python3
"""
claudio screenshot generator
============================
Produces 4 PNG screenshots for the claudio-releases public README.

Usage:
    python3 scripts/gen_screenshots.py [--out release-repo/screenshots]

Requirements:
    - Python 3.8+
    - inkscape or magick (ImageMagick) in PATH

Privacy: all content is hardcoded demo data. A grep at the end verifies no
real user paths or hostnames appear in the SVG source.
"""

import argparse
import html
import os
import subprocess
import sys
import tempfile
from pathlib import Path

# ── palette ──────────────────────────────────────────────────────────────────

BG        = "#1e1e2e"  # dark background
BG2       = "#181825"  # slightly darker (window chrome)
FG        = "#cdd6f4"  # default text
DIM       = "#6c7086"  # dimmed text
CYAN      = "#89dceb"
GREEN     = "#a6e3a1"
YELLOW    = "#f9e2af"
RED       = "#f38ba8"
MAGENTA   = "#cba6f7"
BLUE      = "#89b4fa"
ORANGE    = "#fab387"
REV_BG    = "#cdd6f4"  # reversed bg (active tab / selected row)
REV_FG    = "#1e1e2e"  # reversed fg

# ── geometry ─────────────────────────────────────────────────────────────────

COLS      = 140
ROWS      = 40
FONT_SIZE = 13           # px; monospace cell
LINE_H    = FONT_SIZE * 1.45
CHAR_W    = FONT_SIZE * 0.601   # approximation for JetBrains Mono / Courier
PAD_X     = 24          # window left/right padding
PAD_TOP   = 48          # room for dots bar
PAD_BOT   = 18

TOTAL_W   = int(COLS * CHAR_W + PAD_X * 2)
TOTAL_H   = int(ROWS * LINE_H + PAD_TOP + PAD_BOT)

# ── SVG helpers ───────────────────────────────────────────────────────────────

def esc(s):
    return html.escape(str(s), quote=True)

def cell_x(col):   return PAD_X + col * CHAR_W
def cell_y(row):   return PAD_TOP + row * LINE_H   # top of cell

def svg_header():
    return f'''<svg xmlns="http://www.w3.org/2000/svg"
     width="{TOTAL_W*2}" height="{TOTAL_H*2}"
     viewBox="0 0 {TOTAL_W} {TOTAL_H}"
     font-family="'JetBrains Mono','Cascadia Code','Fira Code',monospace"
     font-size="{FONT_SIZE}px">
  <defs>
    <style>
      text {{ dominant-baseline: text-before-edge; white-space: pre; }}
    </style>
  </defs>
  <!-- window background -->
  <rect width="{TOTAL_W}" height="{TOTAL_H}" rx="10" ry="10" fill="{BG2}"/>
  <!-- dots bar -->
  <rect x="0" y="0" width="{TOTAL_W}" height="36" rx="10" ry="10" fill="{BG2}"/>
  <rect x="0" y="18" width="{TOTAL_W}" height="18" fill="{BG2}"/>
  <circle cx="20" cy="18" r="6" fill="#ff5f57"/>
  <circle cx="40" cy="18" r="6" fill="#febc2e"/>
  <circle cx="60" cy="18" r="6" fill="#28c840"/>
  <!-- terminal area -->
  <rect x="0" y="36" width="{TOTAL_W}" height="{TOTAL_H-36}" rx="0" ry="0" fill="{BG}"/>
  <rect x="0" y="{TOTAL_H-18}" width="{TOTAL_W}" height="18" rx="0" ry="0" fill="{BG2}"/>
'''

def svg_footer():
    return '</svg>\n'

class Canvas:
    """Collects SVG elements for one screenshot."""

    def __init__(self):
        self._parts = [svg_header()]

    def rect(self, col, row, w_cols, h_rows=1, fill=BG, rx=0):
        x = cell_x(col)
        y = cell_y(row)
        W = w_cols * CHAR_W
        H = h_rows * LINE_H
        self._parts.append(
            f'<rect x="{x:.1f}" y="{y:.1f}" width="{W:.1f}" height="{H:.1f}"'
            f' rx="{rx}" fill="{fill}"/>\n'
        )

    def span(self, col, row, text, color=FG, bold=False, bg=None):
        """Render a fixed-width text span starting at (col, row)."""
        if not text:
            return
        x = cell_x(col)
        y = cell_y(row)
        # background rect if needed
        if bg:
            w = len(text) * CHAR_W
            self._parts.append(
                f'<rect x="{x:.1f}" y="{y:.1f}" width="{w:.1f}" height="{LINE_H:.1f}" fill="{bg}"/>\n'
            )
        weight = 'bold' if bold else 'normal'
        self._parts.append(
            f'<text x="{x:.1f}" y="{y:.1f}" fill="{color}" font-weight="{weight}"'
            f'>{esc(text)}</text>\n'
        )

    def row(self, row_idx, segments, default_bg=BG):
        """
        Render a row of segments.
        segments: list of (text, color, bold, bg_or_None)
        Returns column position after all segments.
        """
        col = 0
        for (text, color, bold, bg) in segments:
            self.span(col, row_idx, text, color=color, bold=bold, bg=bg or default_bg)
            col += len(text)
        return col

    def full_row_bg(self, row_idx, color):
        """Fill an entire row with a background color."""
        self._parts.append(
            f'<rect x="{cell_x(0):.1f}" y="{cell_y(row_idx):.1f}"'
            f' width="{COLS*CHAR_W:.1f}" height="{LINE_H:.1f}" fill="{color}"/>\n'
        )

    def to_svg(self):
        self._parts.append(svg_footer())
        return ''.join(self._parts)


# ── screen building helpers ───────────────────────────────────────────────────

def pad(s, width, align='left'):
    s = str(s)
    if len(s) >= width:
        return s[:width]
    if align == 'right':
        return ' ' * (width - len(s)) + s
    return s + ' ' * (width - len(s))

def tab_seg(index, glyph, label, active=False, attention_color=None):
    """Return (prefix_segs, glyph_segs, suffix_segs) for one tab."""
    prefix = f' {index} '
    suffix = f' {label} '
    if active:
        bg, fg = REV_BG, REV_FG
        return [(prefix, REV_FG, True, REV_BG), (glyph[0], glyph[1], True, REV_BG), (suffix, REV_FG, True, REV_BG)]
    elif attention_color:
        return [(prefix, attention_color, True, None), (glyph[0], attention_color, True, None), (suffix, attention_color, True, None)]
    else:
        return [(prefix, FG, False, None), (glyph[0], glyph[1], False, None), (suffix, FG, False, None)]


# ── screenshot 1: sessions view ───────────────────────────────────────────────

def make_sessions():
    c = Canvas()

    # Tab bar (row 0)
    tabs = [
        tab_seg(1, ('⠸', CYAN), 'webapp', active=True),
        tab_seg(2, ('?', YELLOW), 'api-gateway', attention_color=YELLOW),
        tab_seg(3, ('✓', GREEN), 'infra'),
        tab_seg(4, ('✗', RED), 'mobile-app@build-01', attention_color=RED),
    ]
    col = 0
    for i, tab in enumerate(tabs):
        if i > 0:
            c.span(col, 0, '│', color=DIM)
            col += 1
        for seg in tab:
            c.span(col, 0, seg[0], color=seg[1], bold=seg[2], bg=seg[3])
            col += len(seg[0])

    # Session pane: show realistic claude output for 'webapp' (rows 1..38)
    claude_lines = [
        ('', FG, False),
        ('  ╭─ Claude Code ─────────────────────────────────────────────────────────╮', DIM, False),
        ('  │                                                                        │', DIM, False),
        (f'  │  > add input validation to the signup form                            │', FG, False),
        ('  │                                                                        │', DIM, False),
        ('  ╰────────────────────────────────────────────────────────────────────────╯', DIM, False),
        ('', FG, False),
        ("  I'll add input validation to the signup form. Let me look at the current", FG, False),
        ("  implementation first.", FG, False),
        ('', FG, False),
        ('  ● Read src/components/SignupForm.tsx', CYAN, False),
        ('', FG, False),
        ('  The form currently has no validation. I\'ll add:', FG, False),
        ('    • Email format check (RFC 5321)', FG, False),
        ('    • Password length ≥ 8 characters', FG, False),
        ('    • Password confirmation match', FG, False),
        ('    • Required field markers', FG, False),
        ('', FG, False),
        ('  ● Edit src/components/SignupForm.tsx', CYAN, False),
        ('', FG, False),
        ('  ╭────────────────────────────────────────────────────────────────────────╮', DIM, False),
        ('  │  + import { z } from "zod";                                            │', GREEN, False),
        ('  │  + import { useForm } from "react-hook-form";                          │', GREEN, False),
        ('  │  + import { zodResolver } from "@hookform/resolvers/zod";              │', GREEN, False),
        ('  │                                                                        │', DIM, False),
        ('  │  + const signupSchema = z.object({                                     │', GREEN, False),
        ('  │  +   email: z.string().email("Invalid email address"),                 │', GREEN, False),
        ('  │  +   password: z.string().min(8, "At least 8 characters"),             │', GREEN, False),
        ('  │  +   confirm: z.string(),                                              │', GREEN, False),
        ('  │  + }).refine(d => d.password === d.confirm, {                          │', GREEN, False),
        ('  │  +   message: "Passwords do not match", path: ["confirm"],             │', GREEN, False),
        ('  │  + });                                                                  │', GREEN, False),
        ('  ╰────────────────────────────────────────────────────────────────────────╯', DIM, False),
        ('', FG, False),
        ('  ● Write src/components/SignupForm.tsx', CYAN, False),
        ('', FG, False),
        ('  Validation added. Run npm test to verify.            ⠸', CYAN, False),
        ('', FG, False),
    ]

    for i, (text, color, bold) in enumerate(claude_lines[:37]):
        c.span(0, i + 1, text, color=color, bold=bold)

    # Status bar (row 39)
    c.full_row_bg(39, REV_BG)
    status_left = ' ~/projects/webapp · working'
    hints = 'Alt+← prev  Alt+→ next  Alt+n new  Alt+g overview  Alt+q quit '
    pad_len = COLS - len(status_left) - len(hints)
    c.span(0, 39, status_left, color=REV_FG, bold=False, bg=REV_BG)
    c.span(len(status_left), 39, ' ' * pad_len, color=REV_FG, bg=REV_BG)
    c.span(len(status_left) + pad_len, 39, hints, color=DIM, bg=REV_BG)

    return c.to_svg()


# ── screenshot 2: wizard (step 0 - host/start screen) ────────────────────────

def make_wizard():
    c = Canvas()

    # Dim background (one session exists in background)
    c.span(0, 0, f' 1  ✓ webapp ', color=FG)

    # Status bar
    c.full_row_bg(39, REV_BG)
    status = ' ~/projects/webapp · idle'
    hints = 'Alt+← prev  Alt+→ next  Alt+n new  Alt+g overview  Alt+q quit '
    pad_len = COLS - len(status) - len(hints)
    c.span(0, 39, status, color=REV_FG, bg=REV_BG)
    c.span(len(status), 39, ' ' * pad_len, color=REV_FG, bg=REV_BG)
    c.span(len(status) + pad_len, 39, hints, color=DIM, bg=REV_BG)

    # Popup overlay ─ centered, ~90 wide × 22 tall
    PW = 90
    PH = 22
    PC = (COLS - PW) // 2
    PR = (ROWS - PH) // 2

    # Popup background
    for r in range(PH):
        c.full_row_bg(PR + r, BG2)
        c.span(PC, PR + r, ' ' * PW, color=FG, bg=BG2)

    # Popup border (top title)
    title = 'New session (↑/↓ move · Tab switch section · Enter pick · Esc cancel)'
    border_top = '┌' + ('─' * (PW - 2)) + '┐'
    c.span(PC, PR, border_top, color=DIM, bg=BG2)
    title_x = PC + (PW - len(title)) // 2
    c.span(title_x, PR, title, color=CYAN, bold=True, bg=BG2)

    # Bottom border
    border_bot = '└' + ('─' * (PW - 2)) + '┘'
    c.span(PC, PR + PH - 1, border_bot, color=DIM, bg=BG2)

    # Content rows (within popup, offset by 1 for border)
    inner_x = PC + 1
    inner_w = PW - 2
    row = PR + 1

    # Filter line
    c.span(inner_x, row, 'filter> ', color=DIM, bg=BG2)
    c.span(inner_x + 8, row, '▌', color=CYAN, bg=BG2)
    row += 1
    c.span(inner_x, row, '─' * inner_w, color=DIM, bg=BG2)
    row += 1

    # LOCAL section header (focused)
    local_hdr = '── LOCAL ' + '─' * (inner_w - 9)
    c.span(inner_x, row, local_hdr, color=CYAN, bold=True, bg=BG2)
    row += 1

    # Local items
    local_items = [
        ('  Explore local dirs…', FG, False, None),
        ('  ~/projects/webapp', FG, False, REV_BG),  # ← selected (reversed)
        ('  ~/projects/api-gateway', FG, False, None),
        ('  ~/projects/infra', FG, False, None),
        ('  ~/notes', FG, False, None),
    ]
    for (text, color, bold, bg) in local_items:
        if bg == REV_BG:
            c.span(inner_x, row, pad(text, inner_w), color=REV_FG, bold=True, bg=REV_BG)
            # Right-aligned badge for selected item (leave 2-char margin for safety)
            badge = '⎇ main  ✻ 2h ago'
            bx = inner_x + inner_w - len(badge) - 2
            c.span(bx, row, badge, color=REV_FG, bold=False, bg=REV_BG)
        else:
            c.span(inner_x, row, text, color=color, bold=bold, bg=BG2)
            # Add badges to some items
            if 'api-gateway' in text:
                badge = '⎇ feat/rate-limit  ✻ 3h ago'
                bx = inner_x + inner_w - len(badge)
                c.span(bx, row, badge, color=DIM, bg=BG2)
            elif 'infra' in text:
                badge = '⎇ main'
                bx = inner_x + inner_w - len(badge)
                c.span(bx, row, badge, color=DIM, bg=BG2)
        row += 1

    # REMOTE section header (unfocused/dim)
    remote_hdr = '── REMOTE ' + '─' * (inner_w - 10)
    c.span(inner_x, row, remote_hdr, color=DIM, bg=BG2)
    row += 1

    # Remote items
    remote_items = [
        '  prod-db',
        '  build-01',
        '  staging',
    ]
    for text in remote_items:
        c.span(inner_x, row, text, color=FG, bg=BG2)
        row += 1

    return c.to_svg()


# ── screenshot 3: directory explorer with search ──────────────────────────────

def make_directories():
    c = Canvas()

    # Tab bar
    c.span(0, 0, ' 1  ✓ webapp  ', color=FG)
    c.span(14, 0, '│', color=DIM)
    c.span(15, 0, ' 2  ⠸ api-gateway ', color=CYAN)

    # Status bar
    c.full_row_bg(39, REV_BG)
    status = ' ~/projects/api-gateway · working'
    hints = 'Alt+← prev  Alt+→ next  Alt+n new  Alt+g overview  Alt+q quit '
    pad_len = COLS - len(status) - len(hints)
    c.span(0, 39, status, color=REV_FG, bg=REV_BG)
    c.span(len(status), 39, ' ' * pad_len, color=REV_FG, bg=REV_BG)
    c.span(len(status) + pad_len, 39, hints, color=DIM, bg=REV_BG)

    # Directory picker popup (step 1 of wizard, after selecting LOCAL)
    PW = 90
    PH = 26
    PC = (COLS - PW) // 2
    PR = (ROWS - PH) // 2

    for r in range(PH):
        c.span(PC, PR + r, ' ' * PW, color=FG, bg=BG2)

    title = 'New session: directory (Tab complete · Enter pick · Esc cancel)'
    border_top = '┌' + ('─' * (PW - 2)) + '┐'
    c.span(PC, PR, border_top, color=DIM, bg=BG2)
    title_x = PC + (PW - len(title)) // 2
    c.span(title_x, PR, title, color=CYAN, bold=True, bg=BG2)

    border_bot = '└' + ('─' * (PW - 2)) + '┘'
    c.span(PC, PR + PH - 1, border_bot, color=DIM, bg=BG2)

    inner_x = PC + 1
    inner_w = PW - 2
    row = PR + 1

    # Input line with search text
    c.span(inner_x, row, '> ', color=DIM, bg=BG2)
    c.span(inner_x + 2, row, '~/projects/api', color=FG, bold=True, bg=BG2)
    c.span(inner_x + 16, row, '▌', color=CYAN, bg=BG2)
    row += 1
    c.span(inner_x, row, '─' * inner_w, color=DIM, bg=BG2)
    row += 1

    # Directory entries
    entries = [
        # (path, git_branch, claude_at, selected)
        ('~/projects/api-gateway', 'feat/rate-limit', '3h ago', True),
        ('~/projects/api-gateway/src', None, None, False),
        ('~/projects/api-gateway/src/middleware', None, None, False),
        ('~/projects/api-gateway/src/handlers', None, None, False),
        ('~/projects/api-gateway/tests', None, None, False),
        ('~/projects/api-gateway/docs', None, None, False),
        ('~/projects/api-gateway/.github', None, None, False),
    ]

    for (path, branch, claude_at, selected) in entries:
        if selected:
            bg = REV_BG
            fg = REV_FG
        else:
            bg = BG2
            fg = FG

        # Highlight matching part of path
        c.span(inner_x, row, '  ', color=fg, bg=bg)
        text_col = inner_x + 2

        match_str = 'api'
        if match_str in path and not selected:
            # Find and highlight the matching portion
            idx = path.find(match_str)
            c.span(text_col, row, path[:idx], color=fg, bg=bg)
            c.span(text_col + idx, row, match_str, color=YELLOW, bold=True, bg=bg)
            c.span(text_col + idx + len(match_str), row, path[idx+len(match_str):], color=fg, bg=bg)
        else:
            c.span(text_col, row, path, color=fg, bold=selected, bg=bg)

        # Right-aligned badges
        badge_parts = []
        if branch:
            badge_parts.append(('⎇ ' + branch, GREEN if selected else GREEN))
        if claude_at:
            badge_parts.append(('  ✻ ' + claude_at, CYAN if selected else DIM))

        badge_x = inner_x + inner_w
        for (badge_text, badge_color) in reversed(badge_parts):
            badge_x -= len(badge_text)
            c.span(badge_x, row, badge_text, color=REV_FG if selected else badge_color, bg=bg)

        # Fill remainder of row
        path_end = text_col + len(path)
        gap = badge_x - path_end
        if gap > 0:
            c.span(path_end, row, ' ' * gap, color=fg, bg=bg)

        row += 1

    # A few dim entries below
    more = [
        '~/projects/mobile-app',
        '~/projects/infra',
        '~/notes',
    ]
    for p in more:
        c.span(inner_x, row, '  ' + p, color=DIM, bg=BG2)
        row += 1

    return c.to_svg()


# ── screenshot 4: overview popup ──────────────────────────────────────────────

def make_overview():
    c = Canvas()

    # Tab bar
    col = 0
    tab_data = [
        (1, '⠸', CYAN, 'webapp', True),
        (2, '?', YELLOW, 'api-gateway', False),
        (3, '✓', GREEN, 'infra', False),
        (4, '✗', RED, 'mobile-app@build-01', False),
    ]
    for (idx, glyph_ch, glyph_color, label, active) in tab_data:
        if idx > 1:
            c.span(col, 0, '│', color=DIM)
            col += 1
        if active:
            prefix = f' {idx} '
            glyph = glyph_ch
            suffix = f' {label} '
            c.span(col, 0, prefix, color=REV_FG, bold=True, bg=REV_BG)
            col += len(prefix)
            c.span(col, 0, glyph, color=REV_FG, bold=True, bg=REV_BG)
            col += 1
            c.span(col, 0, suffix, color=REV_FG, bold=True, bg=REV_BG)
            col += len(suffix)
        else:
            attn = YELLOW if glyph_ch == '?' else (RED if glyph_ch in ('✗', '◆') else None)
            prefix = f' {idx} '
            glyph = glyph_ch
            suffix = f' {label} '
            fg = attn or FG
            c.span(col, 0, prefix, color=fg, bold=attn is not None)
            col += len(prefix)
            c.span(col, 0, glyph, color=glyph_color, bold=attn is not None)
            col += 1
            c.span(col, 0, suffix, color=fg, bold=attn is not None)
            col += len(suffix)

    # Status bar
    c.full_row_bg(39, REV_BG)
    status = ' ~/projects/webapp · working'
    hints = 'Alt+← prev  Alt+→ next  Alt+n new  Alt+g overview  Alt+q quit '
    pad_len = COLS - len(status) - len(hints)
    c.span(0, 39, status, color=REV_FG, bg=REV_BG)
    c.span(len(status), 39, ' ' * pad_len, color=REV_FG, bg=REV_BG)
    c.span(len(status) + pad_len, 39, hints, color=DIM, bg=REV_BG)

    # Overview popup
    PW = 100
    PH = 14
    PC = (COLS - PW) // 2
    PR = (ROWS - PH) // 2

    for r in range(PH):
        c.span(PC, PR + r, ' ' * PW, color=FG, bg=BG2)

    title = 'Overview (↑/↓ select · Enter switch · type to filter · Esc close)'
    border_top = '┌' + ('─' * (PW - 2)) + '┐'
    c.span(PC, PR, border_top, color=DIM, bg=BG2)
    title_x = PC + (PW - len(title)) // 2
    c.span(title_x, PR, title, color=CYAN, bold=True, bg=BG2)

    border_bot = '└' + ('─' * (PW - 2)) + '┘'
    c.span(PC, PR + PH - 1, border_bot, color=DIM, bg=BG2)

    inner_x = PC + 1
    inner_w = PW - 2
    row = PR + 1

    # Filter line
    c.span(inner_x, row, 'filter> ', color=DIM, bg=BG2)
    c.span(inner_x + 8, row, '▌', color=CYAN, bg=BG2)
    row += 1

    # Column header
    header = f' {"#":>2}   {"label":<20} {"location":<30} {"state":>14} {"age":>5}  '
    c.span(inner_x, row, header[:inner_w], color=DIM, bold=False, bg=BG2)
    row += 1
    c.span(inner_x, row, '─' * inner_w, color=DIM, bg=BG2)
    row += 1

    # Session rows
    sessions = [
        (1, '⠸', CYAN,   'webapp',               '~/projects/webapp',          'working',      '12m', True),
        (2, '?', YELLOW, 'api-gateway',           '~/projects/api-gateway',     'needs input',  '3h',  False),
        (3, '✓', GREEN,  'infra',                 '~/projects/infra',           'idle',         '1d',  False),
        (4, '✗', RED,    'mobile-app@build-01',   'build-01:~/projects/mobile', 'error',        '45m', False),
    ]

    for (idx, glyph_ch, glyph_color, label, location, state, age, selected) in sessions:
        if selected:
            bg = REV_BG
            fg = REV_FG
            gc = REV_FG
        else:
            bg = BG2
            fg = YELLOW if state == 'needs input' else (RED if state == 'error' else FG)
            gc = glyph_color

        row_text = f' {idx:>2} '
        c.span(inner_x, row, row_text, color=fg, bold=selected, bg=bg)
        gx = inner_x + len(row_text)
        c.span(gx, row, glyph_ch, color=gc, bold=selected, bg=bg)
        rest = f' {label:<20} {location:<30} {state:>14} {age:>5}  '
        bold_state = state in ('needs input', 'error') and not selected
        c.span(gx + 1, row, f' {label:<20} {location:<30} ', color=fg, bold=selected or bold_state, bg=bg)
        state_col = gx + 1 + 1 + 20 + 1 + 30 + 1
        state_color = (YELLOW if state == 'needs input' else RED if state == 'error' else GREEN if state == 'idle' else CYAN) if not selected else REV_FG
        c.span(state_col, row, f'{state:>14}', color=state_color, bold=True if not selected else False, bg=bg)
        c.span(state_col + 14, row, f' {age:>5}  ', color=fg, bg=bg)

        row += 1

    return c.to_svg()


# ── privacy check ─────────────────────────────────────────────────────────────

BANNED = ['/volumes', 'p4u', 'vocdoni', 'z6', 'claude.vocdoni']

def privacy_check(svg_text, name):
    lower = svg_text.lower()
    for token in BANNED:
        if token.lower() in lower:
            print(f'FAIL privacy: "{token}" found in {name}', file=sys.stderr)
            sys.exit(1)
    print(f'  privacy OK: {name}')


# ── render SVG → PNG ──────────────────────────────────────────────────────────

def svg_to_png(svg_path, png_path):
    # Try inkscape first, fall back to magick
    try:
        result = subprocess.run(
            ['inkscape', '--export-type=png', f'--export-filename={png_path}',
             '--export-dpi=144', str(svg_path)],
            capture_output=True, text=True
        )
        if result.returncode == 0:
            return
        print(f'  inkscape failed: {result.stderr.strip()[:200]}', file=sys.stderr)
    except FileNotFoundError:
        pass

    # Fallback: ImageMagick
    result = subprocess.run(
        ['magick', '-background', 'none', '-density', '144', str(svg_path), str(png_path)],
        capture_output=True, text=True
    )
    if result.returncode != 0:
        print(f'  magick failed: {result.stderr.strip()[:200]}', file=sys.stderr)
        sys.exit(1)


# ── main ──────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', default='release-repo/screenshots',
                        help='Output directory for PNG files')
    args = parser.parse_args()

    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)

    shots = [
        ('sessions',    make_sessions),
        ('wizard',      make_wizard),
        ('directories', make_directories),
        ('overview',    make_overview),
    ]

    with tempfile.TemporaryDirectory() as tmpdir:
        for name, fn in shots:
            print(f'generating {name}…')
            svg_text = fn()
            privacy_check(svg_text, name)

            svg_path = Path(tmpdir) / f'{name}.svg'
            png_path = out_dir / f'{name}.png'

            svg_path.write_text(svg_text, encoding='utf-8')
            svg_to_png(svg_path, png_path)
            print(f'  → {png_path}')

    print('done.')


if __name__ == '__main__':
    main()
