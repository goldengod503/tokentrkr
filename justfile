default:
    @just --list

# Build the COSMIC applet (the `cosmic` feature is on by default)
build:
    cargo build --release

# Build the SNI tray instead — overwrites the same target/release/tokentrkr path
build-sni:
    cargo build --release --no-default-features

# Copy the binary and desktop entry into ~/.local (never symlink into target/)
install: build
    install -Dm755 target/release/tokentrkr "$HOME/.local/bin/tokentrkr"
    install -Dm644 resources/com.github.goldengod503.TokenTrkr.desktop "$HOME/.local/share/applications/com.github.goldengod503.TokenTrkr.desktop"

# Install, then restart the COSMIC panel to pick up changes
restart: install
    pkill cosmic-panel || true
