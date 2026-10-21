#!/usr/bin/env bash
# Dorker installer for Linux / WSL.
#
# Run this once (./install.sh). It copies Dorker into ~/dorker, makes it
# runnable, optionally installs Rust and/or runs the setup wizard, and prints
# how to start the control panel. It can't install WSL itself — do that once
# on the Windows side with:  wsl --install  (in an admin PowerShell).

TARGET="$HOME/dorker"
SRC="$(cd "$(dirname "$0")" && pwd)"

echo "=== Dorker installer ==="
echo

# Ask a yes/no question (default No). Auto-No when not interactive.
ask() {
  if [ ! -t 0 ]; then return 1; fi
  printf '%s [y/N]: ' "$1"
  read -r a
  case "$a" in [yY]*) return 0 ;; *) return 1 ;; esac
}

# ---- 1. Place files in ~/dorker -------------------------------------------
if [ "$SRC" = "$TARGET" ]; then
  echo "Installing in place ($TARGET)."
else
  echo "Installing Dorker into $TARGET ..."
  mkdir -p "$TARGET"
  # Always refresh the program and helper files.
  for f in dorker panel.sh Dorker-Panel.bat RUN.md install.sh; do
    [ -e "$SRC/$f" ] && cp "$SRC/$f" "$TARGET/"
  done
  # Don't clobber data the user may have edited.
  for f in keywords.json config.toml candidates.txt; do
    if [ -e "$SRC/$f" ] && [ ! -e "$TARGET/$f" ]; then
      cp "$SRC/$f" "$TARGET/"
    elif [ -e "$TARGET/$f" ]; then
      echo "  keeping your existing $f"
    fi
  done
fi

chmod +x "$TARGET/dorker" 2>/dev/null || true
chmod +x "$TARGET/panel.sh" 2>/dev/null || true
chmod +x "$TARGET/install.sh" 2>/dev/null || true

# ---- 2. Does the prebuilt binary run here? --------------------------------
BINARY_OK=
if [ -x "$TARGET/dorker" ] && "$TARGET/dorker" --version >/dev/null 2>&1; then
  echo "The prebuilt dorker binary works on this system."
  BINARY_OK=1
else
  echo "No working prebuilt binary here (your Linux may differ from the build host)."
fi

# ---- 3. Rust (optional) ----------------------------------------------------
echo
if command -v cargo >/dev/null 2>&1; then
  echo "Rust is already installed ($(cargo --version))."
  HAVE_CARGO=1
else
  if [ -z "$BINARY_OK" ]; then
    echo "You'll need Rust to build Dorker from source since the prebuilt binary didn't run."
  fi
  if ask "Install Rust now (rustup) so you can build/update from source?"; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
      && . "$HOME/.cargo/env" && HAVE_CARGO=1 && echo "Rust installed."
  fi
fi

# ---- 4. Build from source if we can and need/want to ----------------------
if [ -f "$TARGET/Cargo.toml" ] && [ -n "$HAVE_CARGO" ]; then
  if [ -z "$BINARY_OK" ] || ask "Build dorker from source now?"; then
    echo "Building (first build takes a few minutes)..."
    ( cd "$TARGET" && cargo build --release && cp target/release/dorker "$TARGET/dorker" ) \
      && chmod +x "$TARGET/dorker" && BINARY_OK=1 && echo "Built."
  fi
elif [ -z "$BINARY_OK" ] && [ ! -f "$TARGET/Cargo.toml" ]; then
  echo
  echo "To build from source, get the code first:"
  echo "  git clone https://github.com/Rusted-1/Dorking_For_Security.git"
  echo "  cd Dorking_For_Security && ./install.sh"
fi

# ---- 5. Setup wizard (optional) -------------------------------------------
echo
if [ -n "$BINARY_OK" ] && ask "Run the setup wizard now (Google key, etc.)?"; then
  ( cd "$TARGET" && ./dorker setup )
fi

# ---- 6. Done ---------------------------------------------------------------
echo
echo "=== Done. Dorker is in $TARGET ==="
if [ -n "$BINARY_OK" ]; then
  echo "Start the control panel (buttons in your browser):"
  echo "  cd ~/dorker && ./panel.sh"
  echo "  (or double-click Dorker-Panel.bat from Windows)"
  echo "Scan one site from the terminal:"
  echo "  cd ~/dorker && ./dorker scan --url https://suspect-site.com/"
else
  echo "Finish by getting a working binary (build from source above), then re-run ./install.sh."
fi
