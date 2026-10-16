#!/usr/bin/env bash
# Registra o Maestro no menu/dock do usuário (ícone + .desktop). Idempotente; rode depois do `cargo build --release`.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"

install -Dm644 "$ROOT/assets/icons/maestro.svg" "$DATA/icons/hicolor/scalable/apps/maestro.svg"
install -Dm644 "$ROOT/assets/icons/maestro-512.png" "$DATA/icons/hicolor/512x512/apps/maestro.png"
install -Dm644 /dev/stdin "$DATA/applications/maestro.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Maestro
Comment=IDE AI-first: você rege, a IDE executa
Exec=$ROOT/target/release/maestro
Icon=maestro
Categories=Development;IDE;
StartupWMClass=maestro
DESKTOP

gtk-update-icon-cache -q "$DATA/icons/hicolor" 2>/dev/null || true
update-desktop-database -q "$DATA/applications" 2>/dev/null || true
echo "==> Maestro registrado em $DATA/applications/maestro.desktop"
