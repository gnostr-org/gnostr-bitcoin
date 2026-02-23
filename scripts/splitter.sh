#!/bin/bash

# gnostr-split.sh - Cross-platform Multiplexer & Log Tracker
# Mimics Rust 'directories' crate: ProjectDirs::from("org", "gnostr", "gnostr")

# 1. Detect OS and set the Data Directory (ProjectDirs::data_dir)
case "$(uname -s)" in
    Darwin)
        # macOS: ~/Library/Application Support/org.gnostr.gnostr/bitcoin
        BASE_DATA_DIR="$HOME/Library/Application Support/org.gnostr.gnostr/bitcoin"
        ;;
    Linux)
        # Linux: ~/.local/share/gnostr/bitcoin (standard XDG path)
        # Note: Rust 'directories' often flattens the org name on Linux
        BASE_DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/gnostr/bitcoin"
        ;;
    MINGW*|MSYS*|CYGWIN*)
        # Windows: C:\Users\Name\AppData\Roaming\gnostr\gnostr\data\bitcoin
        BASE_DATA_DIR="$APPDATA/gnostr/gnostr/data/bitcoin"
        ;;
    *)
        echo "Unsupported OS"
        exit 1
        ;;
esac

LOG_PATH="$BASE_DATA_DIR/gnostr-bitcoin.log"
TCPDUMP_CMD="sudo tcpdump -i any udp port 2456"
TAIL_CMD="tail -f \"$LOG_PATH\""

echo "---------------------------------------------------------------"
echo " Target Log: $LOG_PATH"
echo "---------------------------------------------------------------"

# 2. Multiplexer Detection Logic
if [[ -n "$TMUX" ]]; then
    SESSION_TYPE="tmux"
elif [[ "$TERM" == "screen"* ]]; then
    SESSION_TYPE="screen"
else
    SESSION_TYPE="none"
fi

echo "[+] Detected Session: $SESSION_TYPE"

# 3. Execution & Splitting
case $SESSION_TYPE in
    "tmux")
        echo "[!] Splitting tmux pane..."
        # Refresh sudo credentials so the split-pane doesn't hang on a hidden prompt
        sudo -v
        tmux split-window -v "$TCPDUMP_CMD"
        eval "$TAIL_CMD"
        ;;

    "screen")
        echo "[!] Creating screen region..."
        sudo -v
        screen -X split
        screen -X focus
        # Launching bash/zsh inside screen to run the command
        screen -X screen bash -c "$TCPDUMP_CMD; exec $SHELL"
        eval "$TAIL_CMD"
        ;;

    "none")
        echo "[!] No multiplexer. Using backgrounding..."
        sudo -v
        eval "$TCPDUMP_CMD &"
        eval "$TAIL_CMD"
        ;;
esac
