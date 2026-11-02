# Simpu branding

Concept: **the pearl is the prompt**. A dark, elongated freshwater-mussel shell (long and
kidney-shaped, not a scallop) with a pearl that carries a `>_` prompt.

| File | Use |
|---|---|
| `simpu-icon.svg` | Full-colour app icon (light tile, dark shell), 256 grid. Source of the app icons |
| `simpu-icon-1024.png` | Rendered master for `tauri icon` (regenerate icons from this) |
| `alternatives/` | Rejected variants (teal tile) |
| `simpu-icon-mono.svg` | One-colour mark for <= 24 px (favicon, tray); uses `currentColor` |
| `simpu-wordmark.svg` | Mark + lowercase `simpu` |

Palette: tile `#f4f7fb` -> `#d3e1ec`, shell `#2f4a68` -> `#1b2a3a`, lower shell `#294058` -> `#16222f`, rim `#5fb3c9`,
nacre interior `#b3a0da` -> `#7d6ab3`, pearl `#ffffff` -> `#b8a2dc`, prompt `#43366b`.

Regenerate app icons: `ui/node_modules/.bin/tauri icon branding/simpu-icon-1024.png -o crates/simpu-app/icons`
(then delete the android/ios/Square*/StoreLogo/icns outputs; only the PNGs and `icon.ico` are used).

Rules: keep clear space of one pearl radius around the mark; do not recolour the pearl; below 24 px use the mono
mark (no prompt glyph); never stretch the shell.
