#!/bin/sh
# Builds and installs codenotch: binary, user service and Noctalia plugin.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

cargo build --release --manifest-path "$root/Cargo.toml"

install -Dm755 "$root/target/release/codenotch" "$HOME/.local/bin/codenotch"
install -Dm644 "$root/systemd/codenotch.service" "$HOME/.config/systemd/user/codenotch.service"

# Plugin: enlace a la carpeta del repo, así editar los .luau recarga en caliente.
plugins="${XDG_DATA_HOME:-$HOME/.local/share}/noctalia/plugins"
mkdir -p "$plugins"
ln -sfn "$root/noctalia/codenotch" "$plugins/codenotch"

systemctl --user daemon-reload
systemctl --user enable codenotch.service
systemctl --user restart codenotch.service

if command -v noctalia >/dev/null; then
  noctalia msg plugins enable juanmcanchala/codenotch || true
fi

echo
echo "Done. Add the widget to a bar in ~/.config/noctalia/config.toml:"
echo '  [widget.codenotch]'
echo '  type = "juanmcanchala/codenotch:usage"'
echo 'and put "codenotch" in the start/center/end list of that bar.'
