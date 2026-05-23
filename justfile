default:
    @just --list

# Build release binary
build:
    cargo build --release

# Build and restart the COSMIC panel to pick up changes
restart: build
    pkill cosmic-panel || true
