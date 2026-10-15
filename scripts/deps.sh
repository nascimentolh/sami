#!/usr/bin/env bash
# Dependências para compilar o Maestro (GPUI no Linux). O ambiente em volta (Ghostty, herdr, Claude) vem do
# github.com/rafaelmaestro/dotfiles. Idempotente.
set -euo pipefail

PKGS=(build-essential clang cmake pkg-config
      libfontconfig-dev libfreetype-dev libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev
      libx11-xcb-dev libxcb1-dev libzstd-dev libvulkan1 mesa-vulkan-drivers vulkan-tools)

if dpkg -s "${PKGS[@]}" >/dev/null 2>&1; then
    echo "==> pacotes de sistema: tudo já instalado"
else
    SUDO=""; [ "$(id -u)" -ne 0 ] && SUDO="sudo"
    $SUDO apt-get update -qq
    $SUDO apt-get install -y "${PKGS[@]}"
fi

if command -v cargo >/dev/null || [ -x "$HOME/.cargo/bin/cargo" ]; then
    echo "==> rust: $("$HOME/.cargo/bin/rustc" --version 2>/dev/null || rustc --version)"
else
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile default
fi

echo "==> vulkan"
vulkaninfo --summary 2>/dev/null | grep -E "deviceName|driverName" || echo "  vulkaninfo não respondeu: o GPUI precisa de Vulkan"
