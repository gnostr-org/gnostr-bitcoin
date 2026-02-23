#!/bin/zsh

# gnostr-monitor-v2.sh - Verbose tracker for macOS
# Handles multiple PIDs and BSD-style grep

PROCESS_NAME="gnostr-bitcoin"
LOG_PATH="$HOME/Library/Application Support/org.gnostr.gnostr/bitcoin/gnostr-bitcoin.log"

echo "---------------------------------------------------------------"
echo " Monitoring: $PROCESS_NAME"
echo " Time: $(date)"
echo "---------------------------------------------------------------"

# 1. Find the PIDs and format them for lsof (PID1,PID2,PID3)
PIDS=$(pgrep -x "$PROCESS_NAME")

if [ -z "$PIDS" ]; then
    echo "[!] Error: $PROCESS_NAME is not running."
    exit 1
fi

# Convert newline-separated PIDs to comma-separated for lsof -p
FORMATTED_PIDS=$(echo $PIDS | tr '\n' ',' | sed 's/,$//')

echo "[+] Target PIDs: $FORMATTED_PIDS"

# 2. Print Open Files and Library Dependencies
echo "\n### Open Files & Shared Libraries ###"
# We use the comma-separated list here
lsof -p "$FORMATTED_PIDS" | awk '{print $9}' | grep -E '\.(dylib|plist)|gnostr|dev/tty' | sort -u

# 3. Print Active Network Sockets
echo "\n### Active Network Sockets ###"
lsof -nP -i -a -p "$FORMATTED_PIDS"

# 4. Tail the log file
if [ -f "$LOG_PATH" ]; then
    echo "\n### Tailng Log: $LOG_PATH ###"
    echo "--- Last 5 Lines ---"
    tail -n 5 "$LOG_PATH"
else
    echo "\n[!] Log file not found at $LOG_PATH"
fi

echo "\n---------------------------------------------------------------"
echo " Continuous Network Watch (Press Ctrl+C to stop)"
echo "---------------------------------------------------------------"

# 5. Live loop with fixed BSD grep syntax
while true; do
    # Using -E (Extended Regex) for simpler balancing
    netstat -atun | grep -Ei "ESTABLISHED|LISTEN|SYN_SENT" | tail -n 5
    # Optional: Print the hex-style state you requested previously
    echo "count=$(echo "$PIDS" | wc -l | xargs), state=0xa"
    sleep 2
done
