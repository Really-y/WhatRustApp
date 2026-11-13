#!/usr/bin/env bash
# Installs (user-local):
#   walite       — main app: WhatsApp Web with calls on system Chromium (electron41)
#   walite-lite  — WebKitGTK build, lighter but NO calls (WebKitGTK lacks WebRTC on Arch)
set -euo pipefail

BIN_DIR="${HOME}/.local/bin"
LIB_DIR="${HOME}/.local/lib/walite"
APP_DIR="${HOME}/.local/share/applications"

MAIN_BIN="${BIN_DIR}/walite"
LITE_BIN="${BIN_DIR}/walite-lite"
CHROMIUM_APP="${LIB_DIR}/app"
EXT_LIB="${LIB_DIR}/libwalite_ext.so"
MAIN_DESKTOP="${APP_DIR}/walite.desktop"
LITE_DESKTOP="${APP_DIR}/az.walite.WaLite.desktop"
ICON_DIR="${HOME}/.local/share/icons/hicolor"

if [ "${1:-}" = "--uninstall" ]; then
    echo "Uninstalling WaLite..."
    rm -f "${MAIN_BIN}" "${LITE_BIN}" "${EXT_LIB}" "${MAIN_DESKTOP}" "${LITE_DESKTOP}"
    rm -rf "${CHROMIUM_APP}"
    for s in 32 64 128 256 512; do rm -f "${ICON_DIR}/${s}x${s}/apps/walite.png"; done
    rm -f "${ICON_DIR}/scalable/apps/walite.svg"
    rmdir "${LIB_DIR}" 2>/dev/null || true
    echo "Uninstalled. Data preserved: ~/.local/share/walite-chromium, ~/.local/share/walite, ~/.cache/walite"
    exit 0
fi

cd "$(dirname "$0")"
command -v electron41 >/dev/null || { echo "electron41 not found: sudo pacman -S electron41" >&2; exit 1; }

echo "Building walite-lite (cargo build --release --workspace)..."
cargo build --release --workspace

mkdir -p "${BIN_DIR}" "${LIB_DIR}" "${APP_DIR}"
rm -rf "${CHROMIUM_APP}"
cp -r chromium-proto "${CHROMIUM_APP}"
cp -r assets "${CHROMIUM_APP}/assets"
for s in 32 64 128 256 512; do
    install -Dm644 "assets/logo-${s}.png" "${ICON_DIR}/${s}x${s}/apps/walite.png"
done
install -Dm644 assets/logo.svg "${ICON_DIR}/scalable/apps/walite.svg"
gtk-update-icon-cache -q -t "${ICON_DIR}" 2>/dev/null || true
cp target/release/walite "${LITE_BIN}"
cp target/release/libwalite_ext.so "${EXT_LIB}"

rm -f "${MAIN_BIN}"
cat > "${MAIN_BIN}" << EOF
#!/bin/sh
exec electron41 "${CHROMIUM_APP}" "\$@"
EOF
chmod +x "${MAIN_BIN}"

cat > "${MAIN_DESKTOP}" << 'EOF'
[Desktop Entry]
Name=WaLite
Comment=WhatsApp Web (with calls)
Exec=walite
Icon=walite
Terminal=false
Type=Application
Categories=Network;InstantMessaging;
StartupWMClass=walite
EOF

cat > "${LITE_DESKTOP}" << 'EOF'
[Desktop Entry]
Name=WaLite Lite (no calls)
Comment=WhatsApp Web on WebKitGTK, no calls
Exec=walite-lite
Icon=walite
Terminal=false
Type=Application
Categories=Network;InstantMessaging;
StartupWMClass=az.walite.WaLite
EOF

echo "Installed:"
echo "  ${MAIN_BIN}  (with calls, Chromium)"
echo "  ${LITE_BIN}  (no calls, WebKitGTK)"
