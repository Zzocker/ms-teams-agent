#!/bin/sh
# User-local installation; updates replace only application files.
set -eu
cd "$(dirname "$0")"
mkdir -p "$HOME/.local/bin"
# Rename after copying so an app still running does not cause ETXTBSY.
install -m755 ms-teams-agent "$HOME/.local/bin/ms-teams-agent.new"
mv -f "$HOME/.local/bin/ms-teams-agent.new" "$HOME/.local/bin/ms-teams-agent"
install -Dm644 assets/icons/icon.svg "$HOME/.local/share/icons/hicolor/scalable/apps/ms-teams-agent.svg"
mkdir -p "$HOME/.local/share/applications"
# Quote the absolute executable path so launchers do not depend on shell PATH.
python3 - <<'PY'
import os
from pathlib import Path
path = str(Path.home() / '.local/bin/ms-teams-agent')
if any(c in path for c in '\n\r'):
    raise SystemExit('Home directory must not contain newlines')
for character in ('\\', '"', '`', '$'):
    path = path.replace(character, '\\' + character)
# Desktop entry string escaping is applied before Exec quoting is parsed.
path = path.replace('\\', '\\\\').replace('%', '%%')
entry = Path('assets/linux/ms-teams-agent.desktop').read_text()
entry = entry.replace('Exec=ms-teams-agent', f'Exec="{path}"')
(Path.home() / '.local/share/applications/ms-teams-agent.desktop').write_text(entry)
PY
printf '%s\n' 'Teams installed. Open it from your application launcher.'
