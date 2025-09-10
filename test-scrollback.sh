#!/bin/bash

# Test script for scrollback functionality

echo "Testing beach scrollback history feature..."
echo ""
echo "Starting server with test output..."

# Kill any existing beach processes
pkill -f "beach --debug-log" 2>/dev/null

# Start server with lots of output
./target/debug/beach --debug-log /tmp/beach-server.log -- bash -c "
for i in {1..50}; do 
    echo \"Line \$i: This is test output line number \$i\"; 
done; 
echo ''; 
echo 'SCROLLBACK TEST: Try scrolling up with mouse wheel or Shift+PageUp';
echo 'TEXT SELECTION: Hold Shift while dragging to select text';
echo '';
exec bash
" &

SERVER_PID=$!
sleep 2

# Get session URL from log
SESSION_URL=$(grep "Session URL:" /tmp/beach-server.log | tail -1 | awk '{print $3}')

if [ -z "$SESSION_URL" ]; then
    echo "Failed to get session URL"
    kill $SERVER_PID 2>/dev/null
    exit 1
fi

echo "Server started with PID $SERVER_PID"
echo "Session URL: $SESSION_URL"
echo ""
echo "Starting client..."
echo ""
echo "INSTRUCTIONS:"
echo "1. Try scrolling up with mouse wheel - you should be able to see all 50 lines"
echo "2. Try Shift+PageUp/PageDown for larger scrolls"
echo "3. For text selection: Hold Shift while dragging mouse"
echo "4. Press Ctrl+C in client to exit"
echo ""

# Start client
./target/debug/beach --debug-log /tmp/beach-client.log --join "$SESSION_URL"

# Cleanup
kill $SERVER_PID 2>/dev/null
echo "Test completed"