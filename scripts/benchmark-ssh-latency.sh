#!/bin/bash
set -euo pipefail

# SSH latency benchmark script
# Measures round-trip time for echo commands over SSH

ITERATIONS=30
SSH_KEY="$HOME/.ssh/beach-test-singapore.pem"
SSH_HOST="ec2-user@54.179.73.150"
RESULTS_FILE="/tmp/ssh-latency-results.txt"

echo "Running SSH latency benchmark ($ITERATIONS iterations)..."

# Clear previous results
> "$RESULTS_FILE"

for i in $(seq 1 $ITERATIONS); do
  # Use Python for high-resolution timing (cross-platform)
  latency=$(python3 -c "
import time
import subprocess
start = time.time()
subprocess.run(['ssh', '-i', '$SSH_KEY', '$SSH_HOST', '/tmp/echo-test.sh'],
               input=b'test-$i\n', capture_output=True, check=False)
end = time.time()
print(int((end - start) * 1000))
")

  echo "$latency" >> "$RESULTS_FILE"

  # Progress indicator
  if (( i % 10 == 0 )); then
    echo "  Completed $i/$ITERATIONS iterations..."
  fi
done

echo "SSH latency test complete. Results saved to $RESULTS_FILE"

# Calculate statistics
sort -n "$RESULTS_FILE" > /tmp/ssh-sorted.txt
count=$(wc -l < /tmp/ssh-sorted.txt)
p50_idx=$(( count / 2 ))
p95_idx=$(( count * 95 / 100 ))
p99_idx=$(( count * 99 / 100 ))

p50=$(sed -n "${p50_idx}p" /tmp/ssh-sorted.txt)
p95=$(sed -n "${p95_idx}p" /tmp/ssh-sorted.txt)
p99=$(sed -n "${p99_idx}p" /tmp/ssh-sorted.txt)
mean=$(awk '{s+=$1} END {print int(s/NR)}' "$RESULTS_FILE")

echo ""
echo "=== SSH Latency Results ==="
echo "Iterations: $ITERATIONS"
echo "Mean:       ${mean}ms"
echo "P50:        ${p50}ms"
echo "P95:        ${p95}ms"
echo "P99:        ${p99}ms"
echo ""

# Save stats for comparison script
echo "$mean" > /tmp/ssh-mean.txt
echo "$p50" > /tmp/ssh-p50.txt
echo "$p95" > /tmp/ssh-p95.txt
echo "$p99" > /tmp/ssh-p99.txt
