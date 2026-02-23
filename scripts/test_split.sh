#!/bin/bash
# scripts/test_split.sh

# Target directory for builds
TARGET_DIR="$(pwd)/target"
SESSION_NAME="gnostr-nodes"

# Function to display help
show_help() {
    echo "Usage: ./test_split.sh [options]"
    echo ""
    echo "Options:"
    echo "  -h, --help    Show this help message and exit"
    echo "  -n, --name    Set a custom tmux session name (default: gnostr-nodes)"
    echo "  -k, --keep    Keep existing data directories (skip rm -rf)"
    echo ""
    echo "Key tmux commands for this session:"
    echo "  Ctrl+b then o:   Switch focus between Node 1 and Node 2"
    echo "  Ctrl+b then %:   Manual vertical split (to add a 3rd node)"
    echo "  Ctrl+b then d:   Detach (leaves nodes running in background)"
    echo ""
    echo "To re-attach to a detached session:"
    echo "  tmux attach-session -t $SESSION_NAME"
    echo ""
    echo "To stop everything:"
    echo "  tmux kill-session -t $SESSION_NAME"
    echo ""
}

# Cleanup function for the trap
cleanup() {
    echo -e "\n[!] Shutting down nodes..."
    pkill -f gnostr-bitcoin 2>/dev/null
    # Optional: tmux kill-session -t "$SESSION_NAME" 2>/dev/null
}

# Trap signals for graceful exit
trap cleanup SIGINT SIGTERM

# Parse options
KEEP_DATA=false
while [[ "$#" -gt 0 ]]; do
    case $1 in
        -h|--help) show_help; exit 0 ;;
        -n|--name) SESSION_NAME="$2"; shift ;;
        -k|--keep) KEEP_DATA=true ;;
        *) echo "Unknown parameter passed: $1"; show_help; exit 1 ;;
    esac
    shift
done

# Initial cleanup
pkill -f gnostr-bitcoin 2>/dev/null
tmux kill-session -t "$SESSION_NAME" 2>/dev/null
sleep 1

echo "Building gnostr-bitcoin..."
cargo build --target-dir "$TARGET_DIR" || { echo "Build failed."; exit 1; }

# Data directory setup
DATADIR1="$(pwd)/test_data_1"
DATADIR2="$(pwd)/test_data_2"
[ "$KEEP_DATA" = false ] && rm -rf "$DATADIR1" "$DATADIR2"
mkdir -p "$DATADIR1" "$DATADIR2"

# Define Node Commands
CMD1="cargo run --target-dir $TARGET_DIR --bin gnostr-bitcoin -- --datadir $DATADIR1 --listen"
CMD2="sleep 3; cargo run --target-dir $TARGET_DIR --bin gnostr-bitcoin -- --datadir $DATADIR2 --target-peer-addr 127.0.0.1:8333"

# Execution Logic
if command -v tmux >/dev/null 2>&1; then
    echo "Launching tmux dashboard: $SESSION_NAME"
    echo "---------------------------------------------------------------"
    echo " RE-ATTACH: tmux attach-session -t $SESSION_NAME"
    echo " PANE TOGGLE: Ctrl+b then o"
    echo "---------------------------------------------------------------"

    # Create session and first pane
    tmux new-session -d -s "$SESSION_NAME" -n "Nodes" "$CMD1"

    # Create vertical split for second node
    tmux split-window -h -t "$SESSION_NAME" "$CMD2"

    # Attach to the session
    tmux attach-session -t "$SESSION_NAME"
else
    if [[ "$(uname)" == "Darwin" ]]; then
        echo "tmux not found. Using separate Terminal windows..."
        # (Original macOS Terminal fallback logic remains here)
        SCRIPT1="$DATADIR1/run_node1.command"; echo -e "#!/bin/bash\n$CMD1" > "$SCRIPT1"; chmod +x "$SCRIPT1"
        SCRIPT2="$DATADIR2/run_node2.command"; echo -e "#!/bin/bash\n$CMD2" > "$SCRIPT2"; chmod +x "$SCRIPT2"
        open "$SCRIPT1" && open "$SCRIPT2"
    else
        echo "Please install tmux."
    fi
fi
