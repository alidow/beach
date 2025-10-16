#!/bin/bash
set -euo pipefail

# Beach latency benchmark script
# Measures round-trip time for echo commands over Beach
# Uses a persistent Beach session for fair comparison

ITERATIONS=30
SSH_KEY="$HOME/.ssh/beach-test-singapore.pem"
SSH_HOST="ec2-user@54.179.73.150"
RESULTS_FILE="/tmp/beach-latency-results.txt"

echo "Setting up Beach server on Singapore instance..."

# Kill any existing beach processes
ssh -i "$SSH_KEY" "$SSH_HOST" 'pkill -9 beach 2>/dev/null || true; rm -f /tmp/beach-server.log'

# Start beach server on Singapore instance
ssh -i "$SSH_KEY" "$SSH_HOST" 'nohup ~/beach-new/target/release/beach host bash > /tmp/beach-server.log 2>&1 &' >/dev/null 2>&1

# Wait for server to start
sleep 3

# Get session URL from server log (new format: "share url  : https://...")
session_url=$(ssh -i "$SSH_KEY" "$SSH_HOST" 'grep "share url" /tmp/beach-server.log 2>/dev/null | head -1 | awk "{print \$4}"' 2>/dev/null || echo "")

if [[ -z "$session_url" ]]; then
  echo "ERROR: Could not get session URL from server log"
  ssh -i "$SSH_KEY" "$SSH_HOST" 'cat /tmp/beach-server.log'
  exit 1
fi

echo "Beach server started. Session: $session_url"
echo "Running Beach latency benchmark ($ITERATIONS iterations)..."
echo "Note: This includes Beach client startup time per iteration"

# Clear previous results
> "$RESULTS_FILE"

for i in $(seq 1 $ITERATIONS); do
  # Use Python for high-resolution timing
  latency=$(python3 -c "
import time
import subprocess
import sys

start = time.time()
proc = subprocess.run(
    ['bash', '-c', 'echo \"echo test-$i\" | cargo run -q -p beach -- join \"$session_url\" 2>/dev/null | head -5'],
    capture_output=True,
    timeout=15,
    check=False
)
end = time.time()
print(int((end - start) * 1000))
" 2>/dev/null || echo "9999")

  # Only record if we got a valid measurement
  if [[ "$latency" != "9999" ]]; then
    echo "$latency" >> "$RESULTS_FILE"
  fi

  # Progress indicator
  if (( i % 10 == 0 )); then
    echo "  Completed $i/$ITERATIONS iterations..."
  fi
done

echo "Beach latency test complete. Cleaning up..."

# Cleanup
ssh -i "$SSH_KEY" "$SSH_HOST" 'pkill -9 beach 2>/dev/null || true' >/dev/null 2>&1

# Calculate statistics
if [[ ! -s "$RESULTS_FILE" ]]; then
  echo "ERROR: No successful measurements recorded"
  exit 1
fi

sort -n "$RESULTS_FILE" > /tmp/beach-sorted.txt
count=$(wc -l < /tmp/beach-sorted.txt | tr -d ' ')

p50_idx=$(( count / 2 ))
p95_idx=$(( count * 95 / 100 ))
p99_idx=$(( count * 99 / 100 ))

p50=$(sed -n "${p50_idx}p" /tmp/beach-sorted.txt)
p95=$(sed -n "${p95_idx}p" /tmp/beach-sorted.txt)
p99=$(sed -n "${p99_idx}p" /tmp/beach-sorted.txt)
mean=$(awk '{s+=$1} END {print int(s/NR)}' "$RESULTS_FILE")

echo ""
echo "=== Beach Latency Results ==="
echo "Iterations: $count (successful)"
echo "Mean:       ${mean}ms"
echo "P50:        ${p50}ms"
echo "P95:        ${p95}ms"
echo "P99:        ${p99}ms"
echo ""

# Save stats for comparison script
echo "$mean" > /tmp/beach-mean.txt
echo "$p50" > /tmp/beach-p50.txt
echo "$p95" > /tmp/beach-p95.txt
echo "$p99" > /tmp/beach-p99.txt
