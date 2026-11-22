#!/usr/bin/env bash
set -euo pipefail

# One-click installer for medical-server-cloud-linux-x86_64.tar.gz on Linux cloud VMs
INSTALL_DIR="${INSTALL_DIR:-/opt/medical-server}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

echo "[1/4] Creating service user and install directory: ${INSTALL_DIR}"
id -u medical >/dev/null 2>&1 || sudo useradd -r -m -s /usr/sbin/nologin medical
sudo mkdir -p "${INSTALL_DIR}/data/tantivy"

echo "[2/4] Installing medical-server & medical-mcp binaries and assets"
sudo cp "${ROOT_DIR}/medical-server" "${INSTALL_DIR}/medical-server"
sudo chmod +x "${INSTALL_DIR}/medical-server"
if [ -f "${ROOT_DIR}/medical-mcp" ]; then
  sudo cp "${ROOT_DIR}/medical-mcp" "${INSTALL_DIR}/medical-mcp"
  sudo chmod +x "${INSTALL_DIR}/medical-mcp"
  sudo ln -sf "${INSTALL_DIR}/medical-mcp" /usr/local/bin/medical-mcp
fi
sudo cp -r "${ROOT_DIR}/migrations" "${ROOT_DIR}/config" "${ROOT_DIR}/deploy" "${INSTALL_DIR}/"
if [ ! -f "${INSTALL_DIR}/.env" ]; then
  sudo cp "${ROOT_DIR}/deploy/.env.example" "${INSTALL_DIR}/.env"
  echo "  -> Created ${INSTALL_DIR}/.env from template (please edit secrets!)"
fi
sudo chown -R medical:medical "${INSTALL_DIR}"

echo "[3/4] Installing systemd unit"
sudo cp "${ROOT_DIR}/deploy/medical-server.service" /etc/systemd/system/medical-server.service
sudo systemctl daemon-reload
sudo systemctl enable medical-server

echo "[4/4] Starting medical-server"
sudo systemctl restart medical-server
sudo systemctl status medical-server --no-pager || true
echo "Done! Health check: curl http://127.0.0.1:8080/health"
