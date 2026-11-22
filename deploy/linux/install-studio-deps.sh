#!/usr/bin/env bash
set -euo pipefail

# Install runtime shared libraries required by MedicalStudio on Linux desktop
# Note: Prefer installing the official .deb / .rpm / .AppImage package from GitHub Releases,
# which handles dependencies automatically.

echo "[MedicalStudio] Checking Linux desktop runtime dependencies..."

if command -v apt-get >/dev/null 2>&1; then
    sudo apt-get update
    sudo apt-get install -y \
        libwebkit2gtk-4.1-0 \
        libgtk-3-0 \
        libayatana-appindicator3-1 \
        librsvg2-2 \
        ca-certificates
elif command -v dnf >/dev/null 2>&1; then
    sudo dnf install -y \
        webkit2gtk4.1 \
        gtk3 \
        libappindicator-gtk3 \
        librsvg2 \
        ca-certificates
elif command -v pacman >/dev/null 2>&1; then
    sudo pacman -S --needed --noconfirm \
        webkit2gtk-4.1 \
        gtk3 \
        libappindicator-gtk3 \
        librsvg \
        ca-certificates
else
    echo "Unsupported package manager. Please install webkit2gtk-4.1, gtk3, librsvg2 manually,"
    echo "or use the portable MedicalStudio_*.AppImage from GitHub Releases."
    exit 1
fi

echo "[OK] Runtime dependencies installed! You can now run ./medical-studio"
