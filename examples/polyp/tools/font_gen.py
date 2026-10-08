#!/usr/bin/env python3
"""Generates the extended glyph tables of backends/soft/soft/font.kp (Polyp v4 stage 2).

The ASCII table (U+0020..U+007E) is read from font.kp and never changed. Every other glyph is drawn
here as seven rows of five characters (`#` is a lit pixel, row 0 on top) or composed from an ASCII
letter and an accent mark, then written back into font.kp between the two GENERATED markers as
hex-coded columns: one byte per column, bit r set when row r is lit, five bytes per glyph, exactly
the layout the hand-written ASCII table has. Run `python3 tools/font_gen.py` to rewrite
the tables; `--art` prints every glyph as text.

A cell is 5 columns by 7 rows and the pen advances 6, so a capital with an accent cannot keep its 7
rows: an accented capital is the 5-row squash of the plain one (rows 1 and 5 dropped) under a 2-row
mark, which is what the cap-height of the lowercase x-height body (rows 2..6) allows.
"""
import re, sys, os

HERE = os.path.dirname(os.path.abspath(__file__))
FONT_KP = os.path.join(HERE, '..', 'backends', 'soft', 'soft', 'font.kp')

src = open(FONT_KP).read()
m = re.search(r'static FONT: \[u8; 475\] = \[([^\]]*)\]', src)
ASCII = [int(x) for x in m.group(1).replace('\n', ' ').split(',') if x.strip()]
assert len(ASCII) == 475

def cols_to_rows(cols):
    return [''.join('#' if cols[c] >> r & 1 else '.' for c in range(5)) for r in range(7)]

def rows_to_cols(rows):
    assert len(rows) == 7 and all(len(r) == 5 for r in rows), rows
    return [sum((1 << r) for r in range(7) if rows[r][c] == '#') for c in range(5)]

def ascii_rows(ch):
    g = (ord(ch) - 32) * 5
    return cols_to_rows(ASCII[g:g + 5])

BLANK = '.....'

def G(art):
    rows = [r for r in art.split('\n') if r.strip() != '' or r == '']
    rows = [r.strip() for r in art.strip('\n').split('\n')]
    assert len(rows) == 7, (len(rows), art)
    return rows

def pad(rows_top, rows):
    """`rows` placed from row `rows_top`, blank elsewhere."""
    out = [BLANK] * 7
    for i, r in enumerate(rows):
        out[rows_top + i] = r
    return out

def cap5(rows):
    """The 5-row squash of a capital: rows 1 and 5 dropped."""
    return [rows[0], rows[2], rows[3], rows[4], rows[6]]

def flip_v(rows):
    return list(reversed(rows))

def mirror(rows):
    return [r[::-1] for r in rows]

def overlay(a, b):
    return [''.join('#' if x == '#' or y == '#' else '.' for x, y in zip(ra, rb)) for ra, rb in zip(a, b)]

# --- accent marks, two rows each (rows 0 and 1 of the cell) -----------------------------------
TOP = {
    'acute':  ['...#.', '..#..'],
    'grave':  ['.#...', '..#..'],
    'circ':   ['..#..', '.#.#.'],
    'tilde':  ['.##.#', '#..#.'],
    'diaer':  ['.....', '.#.#.'],
    'ring':   ['.##..', '.##..'],
    'caron':  ['.#.#.', '..#..'],
    'macron': ['.....', '.###.'],
    'breve':  ['#...#', '.###.'],
    'dot':    ['.....', '..#..'],
    'dacute': ['..#.#', '.#.#.'],
}
# marks under a letter, on row 6 (the letter is shifted up one row)
BOTTOM = {
    'cedilla': '..##.',
    'ogonek':  '...##',
}

def with_top(base_rows, mark, capital):
    mk = TOP[mark]
    if capital:
        body = cap5(base_rows)
        rows = mk + body
    else:
        rows = [mk[0], mk[1]] + base_rows[2:7]
    return rows

def with_bottom(base_rows, mark, capital):
    if capital:
        # six rows: drop row 4 of the capital
        body = [base_rows[i] for i in (0, 1, 2, 3, 5, 6)]
        return body + [BOTTOM[mark]]
    body = base_rows[2:7]
    return [BLANK] + body + [BOTTOM[mark]]

def dotless_i():
    r = ascii_rows('i')
    r[0] = BLANK
    return r

def base_letter(ch):
    if ch == 'ı':
        return dotless_i()
    return ascii_rows(ch)

def compose(ch, mark):
    capital = ch.isupper()
    b = base_letter(ch)
    if mark in TOP:
        return with_top(b, mark, capital)
    return with_bottom(b, mark, capital)

GLYPHS = {}   # code point -> rows

def put(cp, rows):
    assert cp not in GLYPHS, hex(cp)
    GLYPHS[cp] = rows

# --- Latin-1 Supplement ------------------------------------------------------------------------
put(0xA0, [BLANK] * 7)
put(0xA1, flip_v(ascii_rows('!')))
put(0xA2, G('''
..#..
.####
#.#..
#.#..
#.#..
.####
..#..'''))
put(0xA3, G('''
..##.
.#..#
.#...
####.
.#...
.#...
#####'''))
put(0xA4, G('''
.....
#...#
.###.
.#.#.
.###.
#...#
.....'''))
put(0xA5, G('''
#...#
.#.#.
#####
..#..
#####
..#..
..#..'''))
put(0xA6, G('''
..#..
..#..
..#..
.....
..#..
..#..
..#..'''))
put(0xA7, G('''
.###.
#....
.###.
#...#
.###.
....#
.###.'''))
put(0xA8, pad(0, ['.#.#.']))
put(0xA9, G('''
.###.
#...#
#.##.
#.#..
#.##.
#...#
.###.'''))
def small(ch):
    b = ascii_rows(ch)
    body = b[2:7]
    return [body[0], body[1], body[2], body[4]]
put(0xAA, pad(0, small('a')) )
GLYPHS[0xAA][4] = BLANK
GLYPHS[0xAA][5] = '####.'
put(0xAB, G('''
.....
..#.#
.#.#.
#.#..
.#.#.
..#.#
.....'''))
put(0xAC, pad(2, ['#####', '....#', '....#']))
put(0xAD, ascii_rows('-'))
put(0xAE, G('''
.###.
#...#
#.##.
#.##.
#.#.#
#...#
.###.'''))
put(0xAF, pad(0, ['#####']))
put(0xB0, pad(0, ['.##..', '#..#.', '.##..']))
put(0xB1, G('''
..#..
..#..
#####
..#..
..#..
.....
#####'''))
def sup(ch):
    return pad(0, cap5(ascii_rows(ch)))
put(0xB2, sup('2'))
put(0xB3, sup('3'))
put(0xB4, pad(0, TOP['acute']))
put(0xB5, G('''
.....
.....
#...#
#...#
#...#
#..##
####.'''))
put(0xB6, G('''
..###
.####
.####
..###
..#.#
..#.#
..#.#'''))
put(0xB7, pad(3, ['..#..']))
put(0xB8, pad(5, ['..#..', '.##..']))
put(0xB9, sup('1'))
put(0xBA, pad(0, small('o')))
GLYPHS[0xBA][4] = BLANK
GLYPHS[0xBA][5] = '####.'
put(0xBB, mirror(GLYPHS[0xAB]))
# U+00BC..U+00BE (the vulgar fractions) have no 5x7 drawing here and stay the replacement box.
put(0xBF, flip_v(ascii_rows('?')))
LATIN1_MARKS = {
    0xC0: ('A', 'grave'), 0xC1: ('A', 'acute'), 0xC2: ('A', 'circ'), 0xC3: ('A', 'tilde'),
    0xC4: ('A', 'diaer'), 0xC5: ('A', 'ring'),
    0xC7: ('C', 'cedilla'),
    0xC8: ('E', 'grave'), 0xC9: ('E', 'acute'), 0xCA: ('E', 'circ'), 0xCB: ('E', 'diaer'),
    0xCC: ('I', 'grave'), 0xCD: ('I', 'acute'), 0xCE: ('I', 'circ'), 0xCF: ('I', 'diaer'),
    0xD1: ('N', 'tilde'),
    0xD2: ('O', 'grave'), 0xD3: ('O', 'acute'), 0xD4: ('O', 'circ'), 0xD5: ('O', 'tilde'), 0xD6: ('O', 'diaer'),
    0xD9: ('U', 'grave'), 0xDA: ('U', 'acute'), 0xDB: ('U', 'circ'), 0xDC: ('U', 'diaer'),
    0xDD: ('Y', 'acute'),
    0xE0: ('a', 'grave'), 0xE1: ('a', 'acute'), 0xE2: ('a', 'circ'), 0xE3: ('a', 'tilde'),
    0xE4: ('a', 'diaer'), 0xE5: ('a', 'ring'),
    0xE7: ('c', 'cedilla'),
    0xE8: ('e', 'grave'), 0xE9: ('e', 'acute'), 0xEA: ('e', 'circ'), 0xEB: ('e', 'diaer'),
    0xEC: ('ı', 'grave'), 0xED: ('ı', 'acute'), 0xEE: ('ı', 'circ'), 0xEF: ('ı', 'diaer'),
    0xF1: ('n', 'tilde'),
    0xF2: ('o', 'grave'), 0xF3: ('o', 'acute'), 0xF4: ('o', 'circ'), 0xF5: ('o', 'tilde'), 0xF6: ('o', 'diaer'),
    0xF9: ('u', 'grave'), 0xFA: ('u', 'acute'), 0xFB: ('u', 'circ'), 0xFC: ('u', 'diaer'),
    0xFD: ('y', 'acute'), 0xFF: ('y', 'diaer'),
}
for cp, (ch, mk) in LATIN1_MARKS.items():
    put(cp, compose(ch, mk))
put(0xC6, G('''
.####
#.#..
#.#..
####.
#.#..
#.#..
#.###'''))
put(0xD0, G('''
###..
#..#.
#...#
###.#
#...#
#..#.
###..'''))
put(0xD7, pad(1, ['#...#', '.#.#.', '..#..', '.#.#.', '#...#']))
put(0xD8, G('''
.###.
#..##
#.#.#
#.#.#
##..#
#...#
.###.'''))
put(0xDE, G('''
#....
#....
####.
#...#
#...#
####.
#....'''))
put(0xDF, G('''
.##..
#..#.
#..#.
#.##.
#..##
#...#
#.##.'''))
put(0xE6, G('''
.....
.....
##.#.
..#.#
.####
#.#..
.####'''))
put(0xF0, G('''
.##.#
..##.
.#.#.
#...#
#...#
#...#
.###.'''))
put(0xF7, pad(1, ['..#..', '.....', '#####', '.....', '..#..']))
put(0xF8, G('''
.....
.....
.###.
#..##
#.#.#
##..#
.###.'''))
put(0xFE, G('''
#....
#....
####.
#...#
#...#
####.
#....'''))

# --- Latin Extended-A (the letters Polish, Czech, Slovak, Hungarian, Turkish, Baltic and Romanian
#     text use most; U+0100..U+017F entries not listed here stay the replacement box) -------------
EXTA = {
    0x100: ('A', 'macron'), 0x101: ('a', 'macron'), 0x102: ('A', 'breve'), 0x103: ('a', 'breve'),
    0x104: ('A', 'ogonek'), 0x105: ('a', 'ogonek'),
    0x106: ('C', 'acute'), 0x107: ('c', 'acute'), 0x108: ('C', 'circ'), 0x109: ('c', 'circ'),
    0x10A: ('C', 'dot'), 0x10B: ('c', 'dot'), 0x10C: ('C', 'caron'), 0x10D: ('c', 'caron'),
    0x10E: ('D', 'caron'),
    0x112: ('E', 'macron'), 0x113: ('e', 'macron'), 0x114: ('E', 'breve'), 0x115: ('e', 'breve'),
    0x116: ('E', 'dot'), 0x117: ('e', 'dot'), 0x118: ('E', 'ogonek'), 0x119: ('e', 'ogonek'),
    0x11A: ('E', 'caron'), 0x11B: ('e', 'caron'),
    0x11C: ('G', 'circ'), 0x11D: ('g', 'circ'), 0x11E: ('G', 'breve'), 0x11F: ('g', 'breve'),
    0x120: ('G', 'dot'), 0x121: ('g', 'dot'),
    0x124: ('H', 'circ'),
    0x128: ('I', 'tilde'), 0x129: ('ı', 'tilde'), 0x12A: ('I', 'macron'), 0x12B: ('ı', 'macron'),
    0x12C: ('I', 'breve'), 0x12D: ('ı', 'breve'), 0x12E: ('I', 'ogonek'), 0x12F: ('i', 'ogonek'),
    0x130: ('I', 'dot'), 0x131: ('ı', None),
    0x134: ('J', 'circ'),
    0x139: ('L', 'acute'),
    0x143: ('N', 'acute'), 0x144: ('n', 'acute'), 0x147: ('N', 'caron'), 0x148: ('n', 'caron'),
    0x14C: ('O', 'macron'), 0x14D: ('o', 'macron'), 0x14E: ('O', 'breve'), 0x14F: ('o', 'breve'),
    0x150: ('O', 'dacute'), 0x151: ('o', 'dacute'),
    0x154: ('R', 'acute'), 0x155: ('r', 'acute'), 0x158: ('R', 'caron'), 0x159: ('r', 'caron'),
    0x15A: ('S', 'acute'), 0x15B: ('s', 'acute'), 0x15C: ('S', 'circ'), 0x15D: ('s', 'circ'),
    0x15E: ('S', 'cedilla'), 0x15F: ('s', 'cedilla'), 0x160: ('S', 'caron'), 0x161: ('s', 'caron'),
    0x162: ('T', 'cedilla'), 0x164: ('T', 'caron'),
    0x168: ('U', 'tilde'), 0x169: ('u', 'tilde'), 0x16A: ('U', 'macron'), 0x16B: ('u', 'macron'),
    0x16C: ('U', 'breve'), 0x16D: ('u', 'breve'), 0x16E: ('U', 'ring'), 0x16F: ('u', 'ring'),
    0x170: ('U', 'dacute'), 0x171: ('u', 'dacute'), 0x172: ('U', 'ogonek'), 0x173: ('u', 'ogonek'),
    0x174: ('W', 'circ'), 0x175: ('w', 'circ'), 0x176: ('Y', 'circ'), 0x177: ('y', 'circ'),
    0x178: ('Y', 'diaer'),
    0x179: ('Z', 'acute'), 0x17A: ('z', 'acute'), 0x17B: ('Z', 'dot'), 0x17C: ('z', 'dot'),
    0x17D: ('Z', 'caron'), 0x17E: ('z', 'caron'),
}
for cp, (ch, mk) in EXTA.items():
    if mk is None:
        put(cp, dotless_i())
    else:
        put(cp, compose(ch, mk))
put(0x110, GLYPHS[0xD0])      # D with stroke is the same drawing as eth
put(0x111, G('''
....#
..###
.####
#...#
#...#
#...#
.####'''))
put(0x141, G('''
#....
#....
#.#..
##...
#....
#....
#####'''))
put(0x142, G('''
.##..
..#.#
..##.
.##..
..#..
..#..
.###.'''))
put(0x152, G('''
.####
#.#..
#.#..
#.###
#.#..
#.#..
.####'''))
put(0x153, G('''
.....
.....
.#.##
#.#.#
#.###
#.#..
.#.##'''))

# --- Greek -----------------------------------------------------------------------------------
GREEK_CAPS_FROM_LATIN = {0x391: 'A', 0x392: 'B', 0x395: 'E', 0x396: 'Z', 0x397: 'H', 0x399: 'I', 0x39A: 'K',
                         0x39C: 'M', 0x39D: 'N', 0x39F: 'O', 0x3A1: 'P', 0x3A4: 'T', 0x3A5: 'Y', 0x3A7: 'X'}
for cp, ch in GREEK_CAPS_FROM_LATIN.items():
    put(cp, ascii_rows(ch))
put(0x393, G('''
#####
#....
#....
#....
#....
#....
#....'''))
put(0x394, G('''
..#..
..#..
.#.#.
.#.#.
#...#
#...#
#####'''))
put(0x398, G('''
.###.
#...#
#...#
#####
#...#
#...#
.###.'''))
put(0x39B, G('''
..#..
..#..
.#.#.
.#.#.
#...#
#...#
#...#'''))
put(0x39E, G('''
#####
.....
.###.
.....
.....
.....
#####'''))
put(0x3A0, G('''
#####
#...#
#...#
#...#
#...#
#...#
#...#'''))
put(0x3A3, G('''
#####
#....
.#...
..#..
.#...
#....
#####'''))
put(0x3A6, G('''
..#..
.###.
#.#.#
#.#.#
#.#.#
.###.
..#..'''))
put(0x3A8, G('''
#.#.#
#.#.#
#.#.#
.###.
..#..
..#..
..#..'''))
put(0x3A9, G('''
.###.
#...#
#...#
#...#
.#.#.
.#.#.
##.##'''))
GREEK_LOWER = {
    0x3B1: '''
.....
.....
.##.#
#..#.
#..#.
#..#.
.##.#''',
    0x3B2: '''
.##..
#..#.
#..#.
###..
#..#.
#..#.
###..''',
    0x3B3: '''
.....
.....
#...#
#...#
.#.#.
..#..
..#..''',
    0x3B4: '''
..##.
.#...
.###.
#...#
#...#
#...#
.###.''',
    0x3B5: '''
.....
.....
..##.
.#...
.##..
.#...
..##.''',
    0x3B6: '''
####.
...#.
..#..
.#...
.#...
.#...
..##.''',
    0x3B7: '''
.....
.....
###..
#..#.
#...#
#...#
#...#''',
    0x3B8: '''
.##..
#..#.
#..#.
####.
#..#.
#..#.
.##..''',
    0x3B9: '''
.....
.....
..#..
..#..
..#..
..#.#
...#.''',
    0x3BA: '''
.....
.....
#..#.
#.#..
##...
#.#..
#..#.''',
    0x3BB: '''
#....
.#...
..#..
.#.#.
.#.#.
#...#
#...#''',
    0x3BE: '''
####.
#....
.###.
#....
#....
#....
.###.''',
    0x3C0: '''
.....
.....
#####
.#.#.
.#.#.
.#.#.
.#.##''',
    0x3C3: '''
.....
.....
.####
#..#.
#...#
#...#
.###.''',
    0x3C2: '''
.....
.....
.####
#....
#....
#....
..##.''',
    0x3C4: '''
.....
.....
#####
..#..
..#..
..#..
...##''',
    0x3C5: '''
.....
.....
#...#
#...#
#...#
#...#
.###.''',
    0x3C6: '''
.....
..#..
.###.
#.#.#
#.#.#
.###.
..#..''',
    0x3C8: '''
.....
#.#.#
#.#.#
#.#.#
.###.
..#..
..#..''',
}
for cp, art in GREEK_LOWER.items():
    put(cp, G(art))
put(0x3BC, GLYPHS[0xB5])
put(0x3BD, ascii_rows('v'))
put(0x3BF, ascii_rows('o'))
put(0x3C1, ascii_rows('p'))
put(0x3C7, ascii_rows('x'))
put(0x3C9, ascii_rows('w'))

# --- Cyrillic --------------------------------------------------------------------------------
CYR_CAPS_FROM_LATIN = {0x410: 'A', 0x412: 'B', 0x415: 'E', 0x41A: 'K', 0x41C: 'M', 0x41D: 'H', 0x41E: 'O',
                       0x420: 'P', 0x421: 'C', 0x422: 'T', 0x425: 'X'}
for cp, ch in CYR_CAPS_FROM_LATIN.items():
    put(cp, ascii_rows(ch))
CYR_CAPS = {
    0x411: '''
#####
#....
####.
#...#
#...#
#...#
####.''',
    0x413: '''
#####
#....
#....
#....
#....
#....
#....''',
    0x414: '''
..###
..#.#
..#.#
.#..#
.#..#
#####
#...#''',
    0x416: '''
#.#.#
#.#.#
.###.
..#..
.###.
#.#.#
#.#.#''',
    0x417: '''
.###.
#...#
....#
..##.
....#
#...#
.###.''',
    0x418: '''
#...#
#..##
#.#.#
##..#
#...#
#...#
#...#''',
    0x41B: '''
..###
.#..#
.#..#
.#..#
#...#
#...#
#...#''',
    0x41F: '''
#####
#...#
#...#
#...#
#...#
#...#
#...#''',
    0x423: '''
#...#
#...#
#...#
.####
....#
#...#
.###.''',
    0x424: '''
..#..
.###.
#.#.#
#.#.#
#.#.#
.###.
..#..''',
    0x426: '''
#..#.
#..#.
#..#.
#..#.
#..#.
#####
....#''',
    0x427: '''
#...#
#...#
#...#
.####
....#
....#
....#''',
    0x428: '''
#.#.#
#.#.#
#.#.#
#.#.#
#.#.#
#.#.#
#####''',
    0x429: '''
#.#.#
#.#.#
#.#.#
#.#.#
#.#.#
#####
....#''',
    0x42A: '''
##...
.#...
.#...
.###.
.#..#
.#..#
.###.''',
    0x42B: '''
#...#
#...#
#...#
###.#
#..##
#..##
###.#''',
    0x42C: '''
#....
#....
#....
####.
#...#
#...#
####.''',
    0x42D: '''
.###.
#...#
....#
.####
....#
#...#
.###.''',
    0x42E: '''
#..#.
#.#.#
#.#.#
###.#
#.#.#
#.#.#
#..#.''',
    0x42F: '''
.####
#...#
#...#
.####
..#.#
#..#.
#...#''',
}
for cp, art in CYR_CAPS.items():
    put(cp, G(art))
put(0x401, compose('E', 'diaer'))
put(0x419, with_top(GLYPHS[0x418], 'breve', True))
CYR_LOWER_FROM_LATIN = {0x430: 'a', 0x435: 'e', 0x43E: 'o', 0x440: 'p', 0x441: 'c', 0x443: 'y', 0x445: 'x'}
for cp, ch in CYR_LOWER_FROM_LATIN.items():
    put(cp, ascii_rows(ch))
CYR_LOWER = {
    0x431: '''
####.
#....
####.
#...#
#...#
#...#
.###.''',
    0x432: '''
.....
.....
####.
#...#
####.
#...#
####.''',
    0x433: '''
.....
.....
####.
#....
#....
#....
#....''',
    0x434: '''
.....
.....
..##.
.#.#.
.#.#.
#####
#...#''',
    0x436: '''
.....
.....
#.#.#
.###.
..#..
.###.
#.#.#''',
    0x437: '''
.....
.....
####.
....#
.###.
....#
####.''',
    0x438: '''
.....
.....
#...#
#..##
#.#.#
##..#
#...#''',
    0x43A: '''
.....
.....
#..#.
#.#..
##...
#.#..
#..#.''',
    0x43B: '''
.....
.....
..###
.#..#
.#..#
#...#
#...#''',
    0x43C: '''
.....
.....
#...#
##.##
#.#.#
#...#
#...#''',
    0x43D: '''
.....
.....
#...#
#...#
#####
#...#
#...#''',
    0x43F: '''
.....
.....
#####
#...#
#...#
#...#
#...#''',
    0x442: '''
.....
.....
#####
..#..
..#..
..#..
..#..''',
    0x444: '''
..#..
..#..
.###.
#.#.#
#.#.#
.###.
..#..''',
    0x446: '''
.....
.....
#..#.
#..#.
#..#.
#####
....#''',
    0x447: '''
.....
.....
#...#
#...#
.####
....#
....#''',
    0x448: '''
.....
.....
#.#.#
#.#.#
#.#.#
#.#.#
#####''',
    0x449: '''
.....
.....
#.#.#
#.#.#
#.#.#
#####
....#''',
    0x44A: '''
.....
.....
##...
.#...
.###.
.#..#
.###.''',
    0x44B: '''
.....
.....
#...#
#...#
###.#
#..##
###.#''',
    0x44C: '''
.....
.....
#....
#....
####.
#...#
####.''',
    0x44D: '''
.....
.....
####.
....#
.####
....#
####.''',
    0x44E: '''
.....
.....
#..#.
#.#.#
###.#
#.#.#
#..#.''',
    0x44F: '''
.....
.....
.####
#...#
.####
..#.#
#..#.''',
}
for cp, art in CYR_LOWER.items():
    put(cp, G(art))
put(0x451, compose('e', 'diaer'))
put(0x439, with_top(GLYPHS[0x438], 'breve', False))

# --- punctuation, currency, arrows, box drawing, shapes ----------------------------------------
PUNCT = {
    0x2013: pad(3, ['.###.']),
    0x2014: pad(3, ['#####']),
    0x2018: pad(0, ['..#..', '.#...', '.##..']),
    0x2019: pad(0, ['.##..', '..#..', '.#...']),
    0x201A: pad(4, ['.##..', '..#..', '.#...']),
    0x201C: pad(0, ['.#.#.', '#.#..', '#.#..']),
    0x201D: pad(0, ['..#.#', '..#.#', '.#.#.']),
    0x201E: pad(4, ['..#.#', '..#.#', '.#.#.']),
    0x2020: G('''
..#..
#####
..#..
..#..
..#..
..#..
..#..'''),
    0x2021: G('''
..#..
#####
..#..
..#..
#####
..#..
..#..'''),
    0x2022: pad(2, ['.###.', '.###.', '.###.']),
    0x2026: pad(6, ['#.#.#']),
}
for cp, rows in PUNCT.items():
    put(cp, rows)
put(0x20AC, G('''
..###
.#...
####.
.#...
####.
.#...
..###'''))
ARROWS = {
    0x2190: pad(1, ['..#..', '.#...', '#####', '.#...', '..#..']),
    0x2192: pad(1, ['..#..', '...#.', '#####', '...#.', '..#..']),
    0x2191: G('''
..#..
.###.
#.#.#
..#..
..#..
..#..
..#..'''),
    0x2193: None,
    0x2194: pad(2, ['.#.#.', '#####', '.#.#.']),
    0x2195: G('''
..#..
.###.
..#..
..#..
..#..
.###.
..#..'''),
}
ARROWS[0x2193] = flip_v(ARROWS[0x2191])
for cp, rows in ARROWS.items():
    put(cp, rows)
def vline(c=2):
    return ['..#..'] * 7
BOX = {
    0x2500: pad(3, ['#####']),
    0x2502: ['..#..'] * 7,
    0x250C: ['.....', '.....', '.....', '..###', '..#..', '..#..', '..#..'],
    0x2510: ['.....', '.....', '.....', '###..', '..#..', '..#..', '..#..'],
    0x2514: ['..#..', '..#..', '..#..', '..###', '.....', '.....', '.....'],
    0x2518: ['..#..', '..#..', '..#..', '###..', '.....', '.....', '.....'],
    0x251C: ['..#..', '..#..', '..#..', '..###', '..#..', '..#..', '..#..'],
    0x2524: ['..#..', '..#..', '..#..', '###..', '..#..', '..#..', '..#..'],
    0x252C: ['.....', '.....', '.....', '#####', '..#..', '..#..', '..#..'],
    0x2534: ['..#..', '..#..', '..#..', '#####', '.....', '.....', '.....'],
    0x253C: ['..#..', '..#..', '..#..', '#####', '..#..', '..#..', '..#..'],
    0x2550: pad(2, ['#####', '.....', '#####']),
    0x2551: ['.#.#.'] * 7,
}
for cp, rows in BOX.items():
    put(cp, rows)
SHAPES = {
    0x25A0: pad(1, ['#####'] * 5),
    0x25A1: pad(1, ['#####', '#...#', '#...#', '#...#', '#####']),
    0x25B2: pad(1, ['..#..', '.###.', '.###.', '#####', '#####']),
    0x25BC: pad(1, ['#####', '#####', '.###.', '.###.', '..#..']),
    0x25CB: pad(1, ['.###.', '#...#', '#...#', '#...#', '.###.']),
    0x25CF: pad(1, ['.###.', '#####', '#####', '#####', '.###.']),
}
for cp, rows in SHAPES.items():
    put(cp, rows)

# --- blocks ----------------------------------------------------------------------------------
BOX_GLYPH = [127, 65, 65, 65, 127]
BLOCKS = [
    ('LATIN1', 0xA0, 0xFF),
    ('LATINA', 0x100, 0x17F),
    ('GREEK', 0x391, 0x3C9),
    ('CYRILLIC', 0x400, 0x45F),
    ('PUNCT', 0x2010, 0x2027),
    ('EURO', 0x20AC, 0x20AC),
    ('ARROWS', 0x2190, 0x2195),
    ('BOX', 0x2500, 0x2551),
    ('SHAPES', 0x25A0, 0x25CF),
]
for cp in GLYPHS:
    assert any(lo <= cp <= hi for _, lo, hi in BLOCKS), hex(cp)
    for r in GLYPHS[cp]:
        assert len(r) == 5, (hex(cp), r)
    assert len(GLYPHS[cp]) == 7, (hex(cp), len(GLYPHS[cp]))

def block_bytes(lo, hi):
    out = []
    for cp in range(lo, hi + 1):
        cols = rows_to_cols(GLYPHS[cp]) if cp in GLYPHS else BOX_GLYPH
        out.extend(cols)
    return out

def fmt_table(name, data):
    # One line: an array literal may not break after the opening bracket.
    return 'static %s: [u8; %d] = [%s,]' % (name, len(data), ', '.join('0x%02X' % b for b in data))

def generated():
    parts = []
    for name, lo, hi in BLOCKS:
        data = block_bytes(lo, hi)
        defined = sum(1 for cp in range(lo, hi + 1) if cp in GLYPHS)
        parts.append('// %s U+%04X..U+%04X: %d of %d code points drawn, the rest are the replacement box.'
                     % (name, lo, hi, defined, hi - lo + 1))
        parts.append(fmt_table('FONT_' + name, data))
    return '\n'.join(parts)

if '--art' in sys.argv:
    for cp in sorted(GLYPHS):
        print('U+%04X %s' % (cp, chr(cp)))
        print('\n'.join(GLYPHS[cp]))
    sys.exit(0)

BEGIN = '// BEGIN GENERATED GLYPH TABLES (tools/font_gen.py)'
END = '// END GENERATED GLYPH TABLES'
text = src
if BEGIN in text:
    a = text.index(BEGIN)
    b = text.index(END) + len(END)
    text = text[:a] + BEGIN + '\n' + generated() + '\n' + END + text[b:]
    open(FONT_KP, 'w').write(text)
    print('wrote', FONT_KP, sum(1 for _ in GLYPHS), 'glyphs')
else:
    print('font.kp has no generated-table markers yet', file=sys.stderr)
    sys.exit(1)
