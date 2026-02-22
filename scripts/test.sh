#!/bin/bash
# scripts/test.sh

# Build the project
echo "Building gnostr-bitcoin..."
cargo build ## TODO support additional build flags like --release

if [ $? -ne 0 ]; then
    echo "Build failed."
    exit 1
fi

#BIN="./target/release/gnostr-bitcoin"

#echo "Build successful."
#echo "NOTE: gnostr-bitcoin currently functions as a client-only node (does not listen for incoming connections)."
#echo "Running multiple instances will result in independent clients connecting to the Bitcoin network."
#echo "They will NOT connect to each other locally."
#echo ""
#echo "To run multiple instances without TUI overlap, please open separate terminals and run:"
#echo "$BIN"
#echo ""
#echo "Launching one instance now..."
#$BIN

cargo run --bin gnostr-bitcoin -- ##TODO support additional runtime flags
