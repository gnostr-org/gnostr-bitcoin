#!/bin/bash

# gnostr-boot.sh - Starts a multiplexer session and splits panes
# Mimics Rust 'directories' crate paths

# 1. Path Setup (Cross-Platform)
case "$(uname -s)" in
    Darwin)
        BASE_DATA_DIR="$HOME/Library/Application Support/org.gnostr.gnostr/bitcoin"
        ;;
    Linux)
        BASE_DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/gnostr/bitcoin"
        ;;
    *)
        BASE_DATA_DIR="./"
        ;;
esac

LOG_PATH="$BASE_DATA_DIR/gnostr-bitcoin.log"
TCPDUMP_CMD="sudo tcpdump -i any udp port 2456"
TAIL_CMD="tail -f \"$LOG_PATH\""

# Ensure log exists so tail doesn't error
touch "$LOG_PATH"

# 2. Check for Multiplexer Availability
if command -v tmux >/dev/null 2>&1; then
    echo "[+] Starting TMUX session for gnostr-bitcoin..."
    sudo -v # Cache sudo for the tcpdump pane

    # Create a new session, name it 'gnostr', run tail in the first pane
    tmux new-session -d -s gnostr "$TAIL_CMD"

    # Split the window vertically and run tcpdump
    tmux split-window -v -t gnostr "$TCPDUMP_CMD"

    # Attach to the session
    tmux attach-session -t gnostr

elif command -v screen >/dev/null 2>&1; then
    echo "[+] Starting SCREEN session for gnostr-bitcoin..."
    sudo -v

    # Screen requires a configuration file or a series of -X commands
    # We'll use a temporary screenrc to define the layout
    SCREENRC=$(mktemp)
    echo "split" >> "$SCREENRC"
    echo "screen 0 $TAIL_CMD" >> "$SCREENRC"
    echo "focus" >> "$SCREENRC"
    echo "screen 1 $TCPDUMP_CMD" >> "$SCREENRC"

    screen -c "$SCREENRC" -S gnostr
    rm "$SCREENRC"

else
    echo "[!] No multiplexer found. Installing tmux is recommended."
    sudo -v
    eval "$TCPDUMP_CMD &"
    eval "$TAIL_CMD"
fi
