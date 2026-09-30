"""Format de rendu commun à tous les environnements (`render_mode="scene"`).

Un environnement ne dessine jamais lui-même : `render()` renvoie une scène
faite de primitives simples (rectangles, cercles, texte), sérialisable en
JSON telle quelle. N'importe quel afficheur — un script matplotlib local, la
page de match du site — peut ainsi dessiner n'importe quel environnement
sans rien connaître de ses règles.

Repère : origine en haut à gauche, x vers la droite, y vers le bas, dans les
unités propres à l'environnement (mètres pour le football, cases pour la
cuisine). `width`/`height` donnent l'étendue de la scène, pour la mise à
l'échelle. Les couleurs sont des chaînes CSS.
"""

from __future__ import annotations


def scene(width: float, height: float, shapes: list[dict], background: str = "#ffffff") -> dict:
    return {"width": float(width), "height": float(height), "background": background, "shapes": shapes}


def rect(x: float, y: float, w: float, h: float, fill: str) -> dict:
    """Rectangle plein ; `(x, y)` est son coin haut-gauche."""
    return {"type": "rect", "x": float(x), "y": float(y), "w": float(w), "h": float(h), "fill": fill}


def circle(x: float, y: float, r: float, fill: str) -> dict:
    """Disque plein ; `(x, y)` est son centre."""
    return {"type": "circle", "x": float(x), "y": float(y), "r": float(r), "fill": fill}


def text(x: float, y: float, value: str, fill: str = "#0f172a", size: float = 0.5) -> dict:
    """Texte centré sur `(x, y)` ; `size` est la hauteur des caractères, dans les unités de la scène."""
    return {"type": "text", "x": float(x), "y": float(y), "text": value, "fill": fill, "size": float(size)}
