#!/bin/bash
# scripts/stress_test.sh

TARGET_DIR="$(pwd)/target"
BINARY="$TARGET_DIR/debug/gnostr-bitcoin"
SESSION_NAME="gnostr-cluster"

# 1. Cleanup & Build
pkill -f gnostr-bitcoin 2>/dev/null
tmux kill-session -t "$SESSION_NAME" 2>/dev/null
cargo build --target-dir "$TARGET_DIR" --bin gnostr-bitcoin

# 2. Setup Data Dirs for 4 Nodes
for i in {1..4}; do
    mkdir -p "test_data_$i"
done

# 3. Define Commands
# Node 1: Primary Hub (Listen on 8333)
CMD1="$BINARY --datadir ./test_data_1 --listen"
# Nodes 2-4: Connect to Node 1
CMD2="sleep 2; $BINARY --datadir ./test_data_2 --target-peer-addr 127.0.0.1:8333"
CMD3="sleep 4; $BINARY --datadir ./test_data_3 --target-peer-addr 127.0.0.1:8333"
CMD4="sleep 6; $BINARY --datadir ./test_data_4 --target-peer-addr 127.0.0.1:8333"

# 4. Launch 2x2 Grid in Tmux
echo "[+] Launching 4-Node Cluster..."

# Start Session with Node 1
tmux new-session -d -s "$SESSION_NAME" "$CMD1"

# Split for Node 2 (Right)
tmux split-window -h -t "$SESSION_NAME" "$CMD2"

# Split for Node 3 (Bottom Left)
tmux select-pane -t 0
tmux split-window -v -t "$SESSION_NAME" "$CMD3"

# Split for Node 4 (Bottom Right)
tmux select-pane -t 1
tmux split-window -v -t "$SESSION_NAME" "$CMD4"

# 5. Final Layout & Attach

tmux select-layout -t "$SESSION_NAME" tiled
tmux attach-session -t "$SESSION_NAME"
