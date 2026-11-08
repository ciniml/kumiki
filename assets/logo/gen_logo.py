#!/usr/bin/env python3
"""kumiki のロゴ / アイコンを生成する（標準ライブラリのみ）。

モチーフ: 三本組木（3 本の角材が中心で組み合う組木パズル）を、
ボクセル（立体ドット）でアイソメトリックに描く。
「部品を組み合わせてボードを組む」と「ピクセルグラフィック」の両方を表す。

使い方: python3 gen_logo.py [出力ディレクトリ]   (既定: このスクリプトのあるディレクトリ)
"""
import math
import os
import sys

# ---- 配色 -------------------------------------------------------------
# 3 本の材: 檜(明) / 欅(中) / 胡桃(暗)。背景は藍。アクセントに朱。
WOOD = {
    "x": "#F2D7A0",  # 檜
    "y": "#D69A55",  # 欅
    "z": "#9A5A2E",  # 胡桃
}
INDIGO = "#1F2B47"
INK_LIGHT = "#F5EEDC"  # 生成り
VERMILION = "#E0503A"

SHADE = {"top": 1.00, "left": 0.80, "right": 0.62}


def shade(hex_color, k):
    r, g, b = (int(hex_color[i:i + 2], 16) for i in (1, 3, 5))
    return "#%02X%02X%02X" % tuple(max(0, min(255, round(c * k))) for c in (r, g, b))


# ---- ボクセル ---------------------------------------------------------
ARM = 2  # 中心から各方向に伸びる腕の長さ（ボクセル数）


def voxels():
    """(x, y, z, 材) のリスト。中心は z 材とする。"""
    vs = {(0, 0, 0): "z"}
    for d in range(1, ARM + 1):
        for s in (-1, 1):
            vs[(s * d, 0, 0)] = "x"
            vs[(0, s * d, 0)] = "y"
            vs[(0, 0, s * d)] = "z"
    return vs


COS30 = math.cos(math.radians(30))


def proj(x, y, z, s):
    """アイソメトリック投影。視点は (+x, +y, +z) 側。"""
    return ((x - y) * COS30 * s, (x + y) * 0.5 * s - z * s)


def cube_faces(x, y, z, s):
    """見える 3 面（上、左=+y 面、右=+x 面）の頂点列。"""
    P = lambda a, b, c: proj(a, b, c, s)
    top = [P(x, y, z + 1), P(x + 1, y, z + 1), P(x + 1, y + 1, z + 1), P(x, y + 1, z + 1)]
    right = [P(x + 1, y, z + 1), P(x + 1, y + 1, z + 1), P(x + 1, y + 1, z), P(x + 1, y, z)]
    left = [P(x, y + 1, z + 1), P(x + 1, y + 1, z + 1), P(x + 1, y + 1, z), P(x, y + 1, z)]
    return {"top": top, "left": left, "right": right}


def burr_svg_group(s, ox, oy, stroke):
    """三本組木の <g> 要素と、その外接矩形を返す。"""
    vs = voxels()
    neighbor = {"top": (0, 0, 1), "left": (0, 1, 0), "right": (1, 0, 0)}
    polys = []
    for (x, y, z) in sorted(vs, key=lambda v: (v[0] + v[1] + v[2], v[2])):
        wood = WOOD[vs[(x, y, z)]]
        for face, pts in cube_faces(x, y, z, s).items():
            dx, dy, dz = neighbor[face]
            if (x + dx, y + dy, z + dz) in vs:
                continue  # 隣のボクセルに隠れる面は描かない
            polys.append((pts, shade(wood, SHADE[face])))
    xs = [p[0] for pts, _ in polys for p in pts]
    ys = [p[1] for pts, _ in polys for p in pts]
    minx, miny = min(xs), min(ys)
    w, h = max(xs) - minx, max(ys) - miny
    out = []
    for pts, color in polys:
        d = " ".join("%.2f,%.2f" % (px - minx + ox, py - miny + oy) for px, py in pts)
        out.append('<polygon points="%s" fill="%s" stroke="%s" stroke-width="%.2f" '
                   'stroke-linejoin="round"/>' % (d, color, stroke, s * 0.06))
    return "\n    ".join(out), w, h


# ---- ドット文字（ワードマーク） ---------------------------------------
GLYPHS = {
    "k": ["1000", "1000", "1001", "1010", "1100", "1010", "1001"],
    "u": ["0000", "0000", "1001", "1001", "1001", "1001", "0111"],
    "m": ["00000", "00000", "11010", "10101", "10101", "10101", "10101"],
    "i": ["1", "0", "1", "1", "1", "1", "1"],
}


def wordmark(text, px, ox, oy, ink, accent):
    rects = []
    x = ox
    for ch in text:
        g = GLYPHS[ch]
        for r, row in enumerate(g):
            for c, bit in enumerate(row):
                if bit == "1":
                    color = accent if (ch == "i" and r == 0) else ink  # i の点だけ朱
                    rects.append('<rect x="%d" y="%d" width="%d" height="%d" fill="%s"/>'
                                 % (x + c * px, oy + r * px, px, px, color))
        x += (len(g[0]) + 1) * px
    width = x - ox - px
    return "\n    ".join(rects), width, 7 * px


def svg(w, h, body, title):
    return ('<svg xmlns="http://www.w3.org/2000/svg" width="%d" height="%d" '
            'viewBox="0 0 %d %d" role="img" aria-label="%s">\n<title>%s</title>\n%s\n</svg>\n'
            % (w, h, w, h, title, title, body))


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.dirname(os.path.abspath(__file__))
    os.makedirs(out, exist_ok=True)
    files = {}

    # 1) シンボル単体（背景透過、正方形）
    size = 512
    g, w, h = burr_svg_group(60, 0, 0, INDIGO)
    g, w, h = burr_svg_group(60, (size - w) / 2, (size - h) / 2, INDIGO)
    files["kumiki-symbol.svg"] = svg(size, size, "  <g>\n    %s\n  </g>" % g, "kumiki")

    # 2) アプリアイコン（藍の角丸正方形）
    g, w, h = burr_svg_group(62, 0, 0, INDIGO)
    g, w, h = burr_svg_group(62, (size - w) / 2, (size - h) / 2, INDIGO)
    body = ('  <rect width="%d" height="%d" rx="%d" fill="%s"/>\n  <g>\n    %s\n  </g>'
            % (size, size, size * 0.22, INDIGO, g))
    files["kumiki-icon.svg"] = svg(size, size, body, "kumiki")

    # 3) 横組みロゴ（シンボル + ドット文字）。明るい背景用と暗い背景用
    for variant, ink, bg in (("light", INDIGO, None), ("dark", INK_LIGHT, INDIGO)):
        pad = 24
        g, w, h = burr_svg_group(26, pad, pad, INDIGO)
        px = 14
        wm_h = 7 * px
        wx = pad + w + 36
        wy = pad + (h - wm_h) / 2 + px * 0.5
        wm, ww, _ = wordmark("kumiki", px, int(wx), int(wy), ink, VERMILION)
        W, H = int(wx + ww + pad), int(h + 2 * pad)
        bg_rect = ('  <rect width="%d" height="%d" rx="18" fill="%s"/>\n' % (W, H, bg)) if bg else ""
        body = "%s  <g>\n    %s\n  </g>\n  <g>\n    %s\n  </g>" % (bg_rect, g, wm)
        files["kumiki-logo-%s.svg" % variant] = svg(W, H, body, "kumiki")

    for name, data in files.items():
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            f.write(data)
        print("wrote", os.path.join(out, name))


if __name__ == "__main__":
    main()
