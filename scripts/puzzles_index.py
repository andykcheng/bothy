"""Writes index.html + game.json for Simon Tatham's puzzles (scripts/third_party_games.sh)."""
import html, json, os, sys

G, VER = sys.argv[1], sys.argv[2]
P = [  # file, title, one line
    ("blackbox", "Black Box", "Fire rays into a box to find the hidden balls."),
    ("bridges", "Bridges", "Link the islands with bridges so each has its number."),
    ("cube", "Cube", "Roll a cube to pick up all the blue squares."),
    ("dominosa", "Dominosa", "Find where a full set of dominoes lies on the grid."),
    ("fifteen", "Fifteen", "Slide the tiles back into order."),
    ("filling", "Filling", "Fill the grid so each region of n cells holds the number n."),
    ("flip", "Flip", "Flip groups of squares to turn them all one colour."),
    ("flood", "Flood", "Flood the grid with one colour in as few moves as you can."),
    ("galaxies", "Galaxies", "Divide the grid into symmetric galaxies around the dots."),
    ("guess", "Guess", "Work out the hidden row of colours from the clues."),
    ("inertia", "Inertia", "Slide around the grid collecting gems, avoiding mines."),
    ("keen", "Keen", "Latin square with arithmetic clues for each cage."),
    ("lightup", "Light Up", "Place lights so every square is lit."),
    ("loopy", "Loopy", "Draw one loop round the grid that fits the numbers."),
    ("magnets", "Magnets", "Place magnets so the + and − counts match."),
    ("map", "Map", "Colour the map with four colours; neighbours must differ."),
    ("mines", "Mines", "Clear the field without setting off a mine."),
    ("mosaic", "Mosaic", "Shade squares to match the numbered clues."),
    ("net", "Net", "Rotate tiles to connect the whole network."),
    ("netslide", "Netslide", "Slide rows and columns to connect the network."),
    ("palisade", "Palisade", "Divide the grid into regions of equal size."),
    ("pattern", "Pattern", "Shade the picture from the row and column clues."),
    ("pearl", "Pearl", "Draw a loop through all the black and white pearls."),
    ("pegs", "Pegs", "Jump pegs over each other until one is left."),
    ("range", "Range", "Shade squares so each number sees that many squares."),
    ("rect", "Rectangles", "Divide the grid into rectangles of the given sizes."),
    ("samegame", "Same Game", "Clear groups of the same colour; bigger groups score more."),
    ("signpost", "Signpost", "Link the squares into one path following the arrows."),
    ("singles", "Singles", "Black out squares so no number repeats in a row or column."),
    ("sixteen", "Sixteen", "Shift rows and columns to put the tiles in order."),
    ("slant", "Slant", "Draw a diagonal in every square, with no loops."),
    ("solo", "Solo", "Sudoku: fill the grid so no digit repeats."),
    ("tents", "Tents", "Put a tent next to every tree."),
    ("towers", "Towers", "Place towers of different heights to match the views."),
    ("tracks", "Tracks", "Lay a railway track from A to B."),
    ("twiddle", "Twiddle", "Rotate blocks of tiles to put them in order."),
    ("undead", "Undead", "Place ghosts, vampires and zombies seen in the mirrors."),
    ("unequal", "Unequal", "Latin square with greater-than clues."),
    ("unruly", "Unruly", "Fill black and white squares with no three in a row."),
    ("untangle", "Untangle", "Move the points until no lines cross."),
]
have = {f[:-5] for f in os.listdir(G) if f.endswith(".html")} - {"index"}
known = {p[0] for p in P}
P = [p for p in P if p[0] in have] + [(n, n.title(), "") for n in sorted(have - known)]  # new upstream puzzles still get listed
rows = "\n".join(f'<a class="p" href="{f}.html"><b>{html.escape(t)}</b><span>{html.escape(d)}</span></a>' for f, t, d in P)
open(f"{G}/index.html", "w").write(f"""<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Puzzles</title>
<style>
 :root {{ color-scheme: light; }}
 body {{ margin: 0; font: 16px system-ui, sans-serif; background: Canvas; color: CanvasText; }}
 header {{ padding: 16px; }} h1 {{ margin: 0 0 4px; font-size: 22px; }} header p {{ margin: 0; opacity: .7; font-size: 14px; }}
 .grid {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(170px, 1fr)); gap: 8px; padding: 0 12px 16px; }}
 .p {{ display: block; padding: 12px; border-radius: 10px; background: color-mix(in srgb, CanvasText 8%, Canvas); color: inherit; text-decoration: none; }}
 .p b {{ display: block; margin-bottom: 4px; }} .p span {{ font-size: 13px; opacity: .75; }}
 footer {{ padding: 8px 16px 24px; font-size: 12px; opacity: .6; }}
</style></head><body>
<header><h1>Puzzles</h1><p>{len(P)} logic puzzles, each with endless levels and several sizes and difficulties (Type menu). Each puzzle's “Full instructions” link has the rules.</p></header>
<div class="grid">
{rows}
</div>
<footer>Simon Tatham's Portable Puzzle Collection (version {html.escape(VER)}), © 2004–2024 Simon Tatham and contributors, MIT licence (see LICENCE). Source: puzzles-source.tar.gz.</footer>
</body></html>
""")
json.dump({"id": "puzzles", "title": "Puzzles",
           "description": f"{len(P)} logic puzzles with endless levels: Mines, Sudoku, Bridges, Loopy, Net, Pattern and more. Quiet, one player, very low power.",
           "players": "1", "version": 1, "license": "MIT",
           "author": "Simon Tatham and contributors (Simon Tatham's Portable Puzzle Collection)", "source": "puzzles-source.tar.gz"},
          open(f"{G}/game.json", "w"), indent=2)
