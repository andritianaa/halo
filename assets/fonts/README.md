# Bundled fonts

Put the `.ttf` files here (SIL OFL 1.1 licence, redistributable):

- **Inter** — https://github.com/rsms/inter/releases (InterVariable.ttf)
- **Space Grotesk** — https://github.com/floriankarsten/space-grotesk/releases
- **JetBrains Mono** — https://github.com/JetBrains/JetBrainsMono/releases

They are loaded at startup (`halo_ui::load_fonts`). Without them, Halo falls back to
Segoe UI Variable / Cascadia Mono / Consolas (see `tokens::FONT_*`).
Font binaries are ignored by git; the installer will copy them next to the exe.
