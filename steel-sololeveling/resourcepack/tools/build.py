#!/usr/bin/env python3
"""Builds the Solo Leveling resource pack.

Every texture is original pixel art drawn here from ASCII templates in the
look of the System: dark navy outlines, cold blue glow, violet for shadows.
No image comes from the anime or the manhwa.

Running the script rewrites `resourcepack/pack/` and the zip next to it:

    python3 steel-sololeveling/resourcepack/tools/build.py

The item ids written here must match `steel-sololeveling/src/item/model.rs`.
"""

import hashlib
import json
import shutil
import sys
import zipfile
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
PACK = ROOT / "pack"
ZIP = ROOT / "sololeveling-resourcepack.zip"
NAMESPACE = "sololeveling"
# Vanilla `RESOURCE_PACK_VERSION` of the targeted client (steel-utils version.rs).
PACK_FORMAT = (97, 1)

OUTLINE = "#0a1030"

# Templates: '.' transparent, 'o' outline, '1' main, '2' highlight,
# '3' accent, '4' handle, '5' handle highlight, 'g' glow.
SWORD = """
.............oo.
............o21o
...........o21o.
..........o21o..
.........o21o...
........o21o....
.......o21o.....
..o...o21o......
..oo.o21o.......
...o333o........
....o33o........
...o4o3o........
..o45o.oo.......
.o45o...........
.o5o............
..o.............
"""

DAGGER = """
................
................
................
............oo..
...........o21o.
..........o21o..
.........o21o...
........o21o....
...o...o21o.....
...oo.o21o......
....o333o.......
.....o33o.......
....o4o3o.......
...o45o.oo......
...o5o..........
....o...........
"""

PICKAXE = """
................
....ooooooo.....
...o2221111o....
..o21oooo111o...
..o1o...o4o11o..
..oo...o4o.o1o..
......o4o...oo..
.....o45o.......
....o45o........
...o45o.........
..o45o..........
.o45o...........
.o5o............
..o.............
................
................
"""

HELMET = """
................
................
.....oooooo.....
....o222111o....
...o22111111o...
...o21111111o...
..o2111111111o..
..o1133333311o..
..o13ggggggg31o.
..o13goooooog3o.
..o11o......o1o.
..o11o......o1o.
...oo........o..
................
................
................
"""

CHESTPLATE = """
................
.ooo........ooo.
o221oooooooo122o
o21111111111112o
.o211113311112o.
..o1111gg1111o..
..o1111gg1111o..
..o1111331111o..
..o1111331111o..
..o1111111111o..
..o1111111111o..
..o1111111111o..
..o1111111111o..
...oooooooooo...
................
................
"""

GLOVE = """
................
................
.....o.o.o......
....o2o2o2o.....
....o1o1o1o.o...
....o1o1o1oo2o..
....o11111o12o..
....o1111111o...
....o111111o....
....o3333331o...
....o3ggggg3o...
....oooooooo....
................
................
................
................
"""

BOOTS = """
................
................
................
...oooo...oooo..
...o21o...o21o..
...o21o...o21o..
...o11o...o11o..
...o11o...o11o..
...o11o...o11o..
...o13oo..o13oo.
..o2133o.o2133o.
..o1ggg3oo1ggg3o
..oooooooooooooo
................
................
................
"""

GEM = """
................
................
......oooo......
.....o2211o.....
....o222111o....
...o22211111o...
..o2221111113o..
..o2211111113o..
...o21111113o...
....o111113o....
.....o11133o....
......o133o.....
.......oo.......
................
................
................
"""

CHEST = """
................
................
..oooooooooooo..
.o222222222221o.
.o211111111111o.
.o211111111111o.
.o333333333333o.
.o3ggggoogggg3o.
.o111111111111o.
.o111111111111o.
.o111111111111o.
.o111111111111o.
.o333333333333o.
..oooooooooooo..
................
................
"""

BOTTLE = """
................
......oooo......
......o44o......
......o54o......
.....oo44oo.....
....o222222o....
...o21111113o...
...o21111113o...
...o2111g113o...
...o211ggg13o...
...o21111113o...
...o11111133o...
....o333333o....
.....oooooo.....
................
................
"""

BOOK = """
................
................
..ooooooooooo...
..o4222222221o..
..o4211111111o..
..o4211ggg111o..
..o421g1g1g11o..
..o4211ggg111o..
..o4211g1g111o..
..o4211111111o..
..o4233333333o..
..o4555555555o..
..ooooooooooooo.
................
................
................
"""

SOUL = """
................
........o.......
.......o2o......
.......o21o.....
......o221o.....
.....o22g11o....
.....o2gg11o....
....o22gg111o...
....o2ggg111o...
....o2gggg11o...
....o21ggg11o...
....o211g111o...
.....o21111o....
......oooooo....
................
................
"""


def palette(main, highlight, accent, handle="#3a2a1c", handle_hi="#6b4f33", glow="#7cf3ff"):
    return {
        "o": OUTLINE,
        "1": main,
        "2": highlight,
        "3": accent,
        "4": handle,
        "5": handle_hi,
        "g": glow,
    }


def draw(template, colours):
    rows = [row for row in template.strip("\n").split("\n")]
    assert len(rows) == 16 and all(len(row) == 16 for row in rows), template
    image = Image.new("RGBA", (16, 16), (0, 0, 0, 0))
    for y, row in enumerate(rows):
        for x, key in enumerate(row):
            if key == ".":
                continue
            colour = colours[key].lstrip("#")
            image.putpixel((x, y), tuple(int(colour[i : i + 2], 16) for i in (0, 2, 4)) + (255,))
    return image


# System colour scheme.
STEEL = palette("#7f93b8", "#d4e4ff", "#3fd6ff")
SHADOW = palette("#3b1c78", "#9b6bff", "#c79bff", glow="#e0c8ff")
DEMON = palette("#4a0f1c", "#ff5c6a", "#ff2d3d", handle="#1a0b12", handle_hi="#5a1a28", glow="#ff9aa2")
GOLD = palette("#c7962a", "#ffe58a", "#3fd6ff")

ITEMS = {
    # Artifacts
    "artifact/kasakas_venom_fang": (DAGGER, palette("#2f9e5a", "#a8ffc8", "#145c34", glow="#7dffb0")),
    "artifact/knights_killer": (SWORD, palette("#8aa0c8", "#eef4ff", "#3a5fb0")),
    "artifact/barukas_dagger": (DAGGER, palette("#c7962a", "#ffe58a", "#2e7dd6")),
    "artifact/demon_kings_daggers": (DAGGER, DEMON),
    "artifact/demon_kings_longsword": (SWORD, DEMON),
    "artifact/archers_gloves": (GLOVE, palette("#6b4a2a", "#b98a55", "#3fd6ff")),
    "artifact/assassins_shoes": (BOOTS, palette("#262d4a", "#5b6a9a", "#3fd6ff")),
    "artifact/high_knights_chestplate": (CHESTPLATE, STEEL),
    "artifact/high_knights_gauntlet": (GLOVE, STEEL),
    "artifact/red_knights_helmet": (HELMET, palette("#9e1c2c", "#ff6b78", "#3a0a12", glow="#ffb3b9")),
    "artifact/crystal_pickaxe": (PICKAXE, palette("#3fb6e8", "#c8f6ff", "#1b3a8a")),
    # Magic crystals by gate rank
    "crystal/e": (GEM, palette("#8a96a8", "#dfe6f0", "#4b5570")),
    "crystal/d": (GEM, palette("#2fae6a", "#a8ffcf", "#13603a")),
    "crystal/c": (GEM, palette("#2f6be0", "#9fd0ff", "#16367a")),
    "crystal/b": (GEM, palette("#7a3fe0", "#d2b0ff", "#3a1a78")),
    "crystal/a": (GEM, palette("#e0a02f", "#fff0a8", "#7a4a10")),
    "crystal/s": (GEM, palette("#e02f4a", "#ffb0bc", "#7a1020")),
    # Reward chests
    "chest/normal": (CHEST, palette("#2a3f7a", "#5b7fd6", "#9aa7bf", glow="#3fd6ff")),
    "chest/sacred": (CHEST, palette("#2a2a4a", "#6a6aa8", "#ffcf3f", glow="#fff3b0")),
    "chest/choice": (CHEST, SHADOW),
    # Buff potions
    "potion/strength": (BOTTLE, palette("#d8283c", "#ff9aa6", "#8a1020", handle="#5b4a3a", handle_hi="#9a8470")),
    "potion/speed": (BOTTLE, palette("#3fd6ff", "#c8f6ff", "#1b6aa8", handle="#5b4a3a", handle_hi="#9a8470")),
    "potion/resistance": (BOTTLE, palette("#7f93b8", "#e4ecff", "#3a4a70", handle="#5b4a3a", handle_hi="#9a8470")),
    "potion/regeneration": (BOTTLE, palette("#ff6bd0", "#ffc8ef", "#8a1a68", handle="#5b4a3a", handle_hi="#9a8470")),
    # Shadow Monarch and the System
    "shadow/soul": (SOUL, SHADOW),
    "system_book": (BOOK, palette("#16295a", "#2f6be0", "#3fd6ff", handle="#0a1030", handle_hi="#c8f6ff")),
}


# Worn armor: vanilla humanoid layer, 64x32. A box at (u, v) of size
# (w, h, d) unfolds into top, bottom, right, front, left and back faces.
def box_faces(u, v, w, h, d):
    return {
        "top": (u + d, v, w, d),
        "bottom": (u + d + w, v, w, d),
        "right": (u, v + d, d, h),
        "front": (u + d, v + d, w, h),
        "left": (u + d + w, v + d, d, h),
        "back": (u + 2 * d + w, v + d, w, h),
    }


HEAD = (0, 0, 8, 8, 8)
BODY = (16, 16, 8, 12, 4)
ARM = (40, 16, 4, 12, 4)
LEG = (0, 16, 4, 12, 4)


def rgb(colour):
    colour = colour.lstrip("#")
    return tuple(int(colour[i : i + 2], 16) for i in (0, 2, 4)) + (255,)


def shade(colour, factor):
    r, g, b, a = rgb(colour)
    return (int(r * factor), int(g * factor), int(b * factor), a)


def paint(image, part, pick):
    """Fills every face of `part` with `pick(face, x, y, width, height)`."""
    for face, (u, v, width, height) in box_faces(*part).items():
        for y in range(height):
            for x in range(width):
                colour = pick(face, x, y, width, height)
                if colour is not None:
                    image.putpixel((u + x, v + y), colour)


def plated(main, light, edge_factor=0.55):
    """Plate armor: lit top rows, dark edges, the main colour between."""

    def pick(face, x, y, width, height):
        if x in (0, width - 1) or y in (0, height - 1):
            return shade(main, edge_factor)
        if face == "top" or y == 1:
            return rgb(light)
        return rgb(main)

    return pick


def lower(rows, inner):
    """Keeps only the bottom `rows` rows of side faces, plus the bottom face."""

    def pick(face, x, y, width, height):
        if face == "top":
            return None
        if face != "bottom" and y < height - rows:
            return None
        return inner(face, x, y, width, height)

    return pick


def rimmed(main, rim, rows):
    """Leather or steel with a glowing rim on the top row of a `rows` band."""

    def pick(face, x, y, width, height):
        if face == "bottom" or x in (0, width - 1):
            return shade(main, 0.6)
        return rgb(rim) if y == height - rows else rgb(main)

    return pick


def red_knights_helmet():
    base = plated("#9e1c2c", "#d8404e")

    def pick(face, x, y, width, height):
        if face == "front" and y in (3, 4) and 1 <= x <= width - 2:
            return rgb("#ffb3b9") if y == 3 else rgb("#2a0810")
        if face == "top" and x in (3, 4):
            return rgb("#ffcf3f")
        return base(face, x, y, width, height)

    image = Image.new("RGBA", (64, 32), (0, 0, 0, 0))
    paint(image, HEAD, pick)
    return image


def high_knights_chestplate():
    steel = plated("#7f93b8", "#d4e4ff")

    def body(face, x, y, width, height):
        if face == "front" and x in (3, 4) and 2 <= y <= 9:
            return rgb("#3fd6ff") if y % 2 == 0 else rgb("#9ff0ff")
        return steel(face, x, y, width, height)

    def pauldron(face, x, y, width, height):
        if face == "bottom" or (face != "top" and y > 4):
            return None
        if face != "top" and y == 4:
            return rgb("#3fd6ff")
        return steel(face, x, y, width, height)

    image = Image.new("RGBA", (64, 32), (0, 0, 0, 0))
    paint(image, BODY, body)
    paint(image, ARM, pauldron)
    return image


def gauntlets(main, rim):
    image = Image.new("RGBA", (64, 32), (0, 0, 0, 0))
    paint(image, ARM, lower(5, rimmed(main, rim, 5)))
    return image


def assassins_shoes():
    image = Image.new("RGBA", (64, 32), (0, 0, 0, 0))
    paint(image, LEG, lower(4, rimmed("#262d4a", "#3fd6ff", 4)))
    return image


# Worn looks of the armor artifacts, keyed like their item models.
EQUIPMENT = {
    "red_knights_helmet": red_knights_helmet,
    "high_knights_chestplate": high_knights_chestplate,
    "high_knights_gauntlet": lambda: gauntlets("#7f93b8", "#3fd6ff"),
    "archers_gloves": lambda: gauntlets("#6b4a2a", "#3fd6ff"),
    "assassins_shoes": assassins_shoes,
}


# Menu button icons, drawn from shapes and outlined automatically.
def shaped(fill, main, light):
    """Fills the pixels `fill(x, y)` accepts, lights their top edge and outlines them."""
    inside = {(x, y) for y in range(16) for x in range(16) if fill(x, y)}
    image = Image.new("RGBA", (16, 16), (0, 0, 0, 0))
    for x, y in inside:
        image.putpixel((x, y), rgb(light) if (x, y - 1) not in inside else rgb(main))
    for y in range(16):
        for x in range(16):
            if (x, y) in inside:
                continue
            if any((x + dx, y + dy) in inside for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1))):
                image.putpixel((x, y), rgb(OUTLINE))
    return image


def arrow(right):
    def fill(x, y):
        x = x if right else 15 - x
        if 3 <= x <= 8 and 6 <= y <= 9:
            return True
        return 8 <= x <= 13 and abs(y - 7.5) <= 13 - x + 0.5

    return shaped(fill, "#2f8fe0", "#9ff0ff")


def check(x, y):
    return (3 <= x <= 6 and abs(y - (x + 5)) <= 1) or (6 <= x <= 12 and abs(y - (17 - x)) <= 1)


def cross(x, y):
    return 3 <= x <= 12 and 3 <= y <= 12 and (abs(x - y) <= 1 or abs(x + y - 15) <= 1)


def plus(x, y):
    return (6 <= x <= 9 and 3 <= y <= 12) or (3 <= x <= 12 and 6 <= y <= 9)


def minus(x, y):
    return 3 <= x <= 12 and 6 <= y <= 9


UI_ICONS = {
    "ui/next": lambda: arrow(True),
    "ui/previous": lambda: arrow(False),
    "ui/confirm": lambda: shaped(check, "#2fae6a", "#a8ffcf"),
    "ui/cancel": lambda: shaped(cross, "#d8283c", "#ff9aa6"),
    "ui/plus": lambda: shaped(plus, "#2f8fe0", "#9ff0ff"),
    "ui/minus": lambda: shaped(minus, "#d8283c", "#ff9aa6"),
}


# Rank badges for chat and the sidebar: a 9x9 plate with the rank letter.
LETTERS = {
    "E": ["111", "1..", "11.", "1..", "111"],
    "D": ["11.", "1.1", "1.1", "1.1", "11."],
    "C": [".11", "1..", "1..", "1..", ".11"],
    "B": ["11.", "1.1", "11.", "1.1", "11."],
    "A": [".1.", "1.1", "111", "1.1", "1.1"],
    "S": [".11", "1..", ".1.", "..1", "11."],
}
RANK_COLOURS = {
    "E": "#8a96a8",
    "D": "#2fae6a",
    "C": "#2f6be0",
    "B": "#7a3fe0",
    "A": "#e0a02f",
    "S": "#e02f4a",
}
# Private-use code points, in rank order; must match src/rank_icon.rs.
RANK_GLYPHS = {rank: chr(0xE101 + index) for index, rank in enumerate("EDCBAS")}


def rank_badge(rank):
    image = Image.new("RGBA", (9, 9), (0, 0, 0, 0))
    colour = RANK_COLOURS[rank]
    for y in range(9):
        for x in range(9):
            corner = x in (0, 8) and y in (0, 8)
            if corner:
                continue
            edge = x in (0, 8) or y in (0, 8)
            if edge:
                image.putpixel((x, y), rgb(OUTLINE))
            else:
                image.putpixel((x, y), shade(colour, 0.75) if y >= 6 else rgb(colour))
    for y, row in enumerate(LETTERS[rank]):
        for x, key in enumerate(row):
            if key == "1":
                image.putpixel((3 + x, 2 + y), rgb("#ffffff"))
    return image


# System sounds, synthesised so no recording of anyone else is used.
SAMPLE_RATE = 44100


def tone(frequency, seconds, decay=6.0, attack=0.005, harmonics=(1.0, 0.35, 0.12)):
    import numpy as np

    time = np.arange(int(SAMPLE_RATE * seconds)) / SAMPLE_RATE
    wave = sum(
        weight * np.sin(2 * np.pi * frequency * (index + 1) * time)
        for index, weight in enumerate(harmonics)
    )
    envelope = np.minimum(time / attack, 1.0) * np.exp(-decay * time)
    return wave * envelope


def sequence(notes, gap):
    import numpy as np

    length = int(SAMPLE_RATE * (gap * (len(notes) - 1) + max(seconds for _, seconds in notes)))
    out = np.zeros(length + SAMPLE_RATE // 10)
    for index, (frequency, seconds) in enumerate(notes):
        start = int(SAMPLE_RATE * gap * index)
        part = tone(frequency, seconds)
        out[start : start + len(part)] += part
    return out


def arise_sound():
    import numpy as np

    seconds = 0.9
    time = np.arange(int(SAMPLE_RATE * seconds)) / SAMPLE_RATE
    sweep = np.sin(2 * np.pi * (70 * time + 40 * time * time))
    rng = np.random.default_rng(7)
    rumble = np.convolve(rng.standard_normal(len(time)), np.ones(60) / 60, mode="same")
    body = (0.8 * sweep + 0.6 * rumble) * np.minimum(time / 0.15, 1.0) * np.exp(-2.5 * time)
    chime = tone(659.25, 0.6, decay=5.0)
    start = int(SAMPLE_RATE * 0.35)
    body[start : start + len(chime)] += 0.5 * chime[: len(body) - start]
    return body


SOUNDS = {
    "open": lambda: sequence([(880.0, 0.25), (1318.5, 0.3)], 0.07),
    "level_up": lambda: sequence([(523.25, 0.3), (659.25, 0.3), (783.99, 0.3), (1046.5, 0.5)], 0.09),
    "quest": lambda: sequence([(783.99, 0.35), (1046.5, 0.6)], 0.12),
    "arise": arise_sound,
    "reward": lambda: sequence([(1567.98, 0.25), (2093.0, 0.35)], 0.06),
}


def write_sound(samples, target):
    import subprocess
    import tempfile
    import wave

    import numpy as np

    peak = float(np.max(np.abs(samples))) or 1.0
    pcm = (samples / peak * 0.8 * 32767).astype(np.int16)
    target.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as directory:
        source = Path(directory) / "sound.wav"
        with wave.open(str(source), "wb") as file:
            file.setnchannels(1)
            file.setsampwidth(2)
            file.setframerate(SAMPLE_RATE)
            file.writeframes(pcm.tobytes())
        subprocess.run(
            ["ffmpeg", "-loglevel", "error", "-y", "-i", str(source), "-map_metadata", "-1",
             "-c:a", "libvorbis", "-q:a", "4", "-fflags", "+bitexact", str(target)],
            check=True,
        )


def write_json(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def build():
    if PACK.exists():
        shutil.rmtree(PACK)
    assets = PACK / "assets" / NAMESPACE
    for item, (template, colours) in ITEMS.items():
        texture = assets / "textures" / "item" / f"{item}.png"
        texture.parent.mkdir(parents=True, exist_ok=True)
        draw(template, colours).save(texture)
        write_json(
            assets / "models" / "item" / f"{item}.json",
            {
                "parent": "minecraft:item/handheld"
                if template in (SWORD, DAGGER, PICKAXE)
                else "minecraft:item/generated",
                "textures": {"layer0": f"{NAMESPACE}:item/{item}"},
            },
        )
        write_json(
            assets / "items" / f"{item}.json",
            {"model": {"type": "minecraft:model", "model": f"{NAMESPACE}:item/{item}"}},
        )
    for name, draw_layer in EQUIPMENT.items():
        layer = assets / "textures" / "entity" / "equipment" / "humanoid" / f"{name}.png"
        layer.parent.mkdir(parents=True, exist_ok=True)
        draw_layer().save(layer)
        write_json(
            assets / "equipment" / f"{name}.json",
            {"layers": {"humanoid": [{"texture": f"{NAMESPACE}:{name}"}]}},
        )
    for item, draw_icon in UI_ICONS.items():
        texture = assets / "textures" / "item" / f"{item}.png"
        texture.parent.mkdir(parents=True, exist_ok=True)
        draw_icon().save(texture)
        write_json(
            assets / "models" / "item" / f"{item}.json",
            {"parent": "minecraft:item/generated", "textures": {"layer0": f"{NAMESPACE}:item/{item}"}},
        )
        write_json(
            assets / "items" / f"{item}.json",
            {"model": {"type": "minecraft:model", "model": f"{NAMESPACE}:item/{item}"}},
        )
    providers = []
    for rank, glyph in RANK_GLYPHS.items():
        badge = assets / "textures" / "font" / f"rank_{rank.lower()}.png"
        badge.parent.mkdir(parents=True, exist_ok=True)
        rank_badge(rank).save(badge)
        providers.append(
            {
                "type": "bitmap",
                "file": f"{NAMESPACE}:font/rank_{rank.lower()}.png",
                "ascent": 8,
                "height": 9,
                "chars": [glyph],
            }
        )
    write_json(assets / "font" / "rank.json", {"providers": providers})
    sounds = {}
    for name, synthesise in SOUNDS.items():
        write_sound(synthesise(), assets / "sounds" / "system" / f"{name}.ogg")
        sounds[f"system.{name}"] = {"sounds": [f"{NAMESPACE}:system/{name}"]}
    write_json(assets / "sounds.json", sounds)
    write_json(
        PACK / "pack.mcmeta",
        {
            "pack": {
                "description": "Solo Leveling — Система",
                "min_format": list(PACK_FORMAT),
                "max_format": list(PACK_FORMAT),
            }
        },
    )
    icon = draw(SOUL, SHADOW).resize((64, 64), Image.NEAREST)
    icon.save(PACK / "pack.png")

    if ZIP.exists():
        ZIP.unlink()
    with zipfile.ZipFile(ZIP, "w", zipfile.ZIP_DEFLATED) as archive:
        for file in sorted(PACK.rglob("*")):
            if file.is_file():
                info = zipfile.ZipInfo(str(file.relative_to(PACK)), date_time=(2026, 1, 1, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                archive.writestr(info, file.read_bytes())
    digest = hashlib.sha1(ZIP.read_bytes()).hexdigest()
    print(
        f"{len(ITEMS)} items, {len(UI_ICONS)} menu icons, {len(EQUIPMENT)} armor layers, "
        f"{len(RANK_GLYPHS)} rank badges, {len(SOUNDS)} sounds -> {ZIP} (sha1 {digest})"
    )


if __name__ == "__main__":
    sys.exit(build())
