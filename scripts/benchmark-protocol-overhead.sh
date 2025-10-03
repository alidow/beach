#!/bin/bash
set -euo pipefail

# Protocol overhead comparison: SSH vs Beach (both localhost)
# This isolates the protocol performance without network latency

ITERATIONS=30

echo "========================================"
echo "  Protocol Overhead: SSH vs Beach"
echo "  (Both running on localhost)"
echo "========================================"
echo ""

# Setup: Start local SSH server test
echo "Setting up test environment..."

# Create a local test script
cat > /tmp/local-echo-test.sh << 'EOF'
#!/bin/bash
while read -r line; do
  echo "$line"
done
EOF
chmod +x /tmp/local-echo-test.sh

# Test 1: SSH to localhost
echo "[1/2] Testing SSH latency (localhost, $ITERATIONS iterations)..."
> /tmp/ssh-local-results.txt

for i in $(seq 1 $ITERATIONS); do
  latency=$(python3 -c "
import time
import subprocess
start = time.time()
subprocess.run(['ssh', 'localhost', '/tmp/local-echo-test.sh'],
               input=b'test-$i\n', capture_output=True, check=False, timeout=10)
end = time.time()
print(int((end - start) * 1000))
" 2>/dev/null || echo "9999")

  if [[ "$latency" != "9999" ]]; then
    echo "$latency" >> /tmp/ssh-local-results.txt
  fi

  if (( i % 10 == 0 )); then
    echo "  Completed $i/$ITERATIONS..."
  fi
done

# Calculate SSH stats
sort -n /tmp/ssh-local-results.txt > /tmp/ssh-local-sorted.txt
ssh_count=$(wc -l < /tmp/ssh-local-sorted.txt | tr -d ' ')

if [[ $ssh_count -eq 0 ]]; then
  echo "ERROR: SSH to localhost failed. Make sure SSH is enabled."
  echo "Run: sudo systemsetup -setremotelogin on"
  exit 1
fi

ssh_p50_idx=$(( ssh_count / 2 ))
ssh_p95_idx=$(( ssh_count * 95 / 100 ))
ssh_p99_idx=$(( ssh_count * 99 / 100 ))

ssh_p50=$(sed -n "${ssh_p50_idx}p" /tmp/ssh-local-sorted.txt)
ssh_p95=$(sed -n "${ssh_p95_idx}p" /tmp/ssh-local-sorted.txt)
ssh_p99=$(sed -n "${ssh_p99_idx}p" /tmp/ssh-local-sorted.txt)
ssh_mean=$(awk '{s+=$1} END {print int(s/NR)}' /tmp/ssh-local-results.txt)

echo "  Mean: ${ssh_mean}ms, P50: ${ssh_p50}ms, P95: ${ssh_p95}ms, P99: ${ssh_p99}ms"
echo ""

# Test 2: Beach localhost
echo "[2/2] Testing Beach latency (localhost, $ITERATIONS iterations)..."

# Start local beach server
cargo run -q -p beach -- -- bash > /tmp/beach-server-local.log 2>&1 &
BEACH_PID=$!

sleep 3

session_url=$(grep -o "beach join [^ ]*" /tmp/beach-server-local.log 2>/dev/null | head -1 | cut -d" " -f3 || echo "")

if [[ -z "$session_url" ]]; then
  echo "ERROR: Could not start Beach server"
  kill $BEACH_PID 2>/dev/null || true
  exit 1
fi

echo "  Session: $session_url"

> /tmp/beach-local-results.txt

for i in $(seq 1 $ITERATIONS); do
  latency=$(python3 -c "
import time
import subprocess
start = time.time()
proc = subprocess.run(
    ['bash', '-c', 'echo \"echo test-$i\" | cargo run -q -p beach -- join \"$session_url\" 2>/dev/null | head -2'],
    capture_output=True,
    timeout=15,
    check=False
)
end = time.time()
print(int((end - start) * 1000))
" 2>/dev/null || echo "9999")

  if [[ "$latency" != "9999" ]]; then
    echo "$latency" >> /tmp/beach-local-results.txt
  fi

  if (( i % 10 == 0 )); then
    echo "  Completed $i/$ITERATIONS..."
  fi
done

# Cleanup
kill $BEACH_PID 2>/dev/null || true

# Calculate Beach stats
sort -n /tmp/beach-local-results.txt > /tmp/beach-local-sorted.txt
beach_count=$(wc -l < /tmp/beach-local-sorted.txt | tr -d ' ')

if [[ $beach_count -eq 0 ]]; then
  echo "ERROR: No successful Beach measurements"
  exit 1
fi

beach_p50_idx=$(( beach_count / 2 ))
beach_p95_idx=$(( beach_count * 95 / 100 ))
beach_p99_idx=$(( beach_count * 99 / 100 ))

beach_p50=$(sed -n "${beach_p50_idx}p" /tmp/beach-local-sorted.txt)
beach_p95=$(sed -n "${beach_p95_idx}p" /tmp/beach-local-sorted.txt)
beach_p99=$(sed -n "${beach_p99_idx}p" /tmp/beach-local-sorted.txt)
beach_mean=$(awk '{s+=$1} END {print int(s/NR)}' /tmp/beach-local-results.txt)

echo "  Mean: ${beach_mean}ms, P50: ${beach_p50}ms, P95: ${beach_p95}ms, P99: ${beach_p99}ms"
echo ""

# Comparison
echo "========================================"
echo "  Results: Protocol Overhead Comparison"
echo "========================================"
printf "%-12s %10s %10s %12s %12s\n" "Metric" "SSH" "Beach" "Difference" "% Diff"
printf "%-12s %10s %10s %12s %12s\n" "------" "---" "-----" "----------" "------"

for metric in "Mean:mean" "P50:p50" "P95:p95" "P99:p99"; do
  name=$(echo $metric | cut -d: -f1)
  var=$(echo $metric | cut -d: -f2)

  ssh_val=$(eval echo \$ssh_$var)
  beach_val=$(eval echo \$beach_$var)
  diff=$((ssh_val - beach_val))
  pct=$(awk "BEGIN {printf \"%.1f\", ($diff / $ssh_val) * 100}")

  printf "%-12s %9dms %9dms %11dms %11s%%\n" "$name" "$ssh_val" "$beach_val" "$diff" "$pct"
done

echo ""
echo "Interpretation:"
echo "  - Positive % = Beach is faster"
echo "  - Negative % = SSH is faster"
echo "  - This measures pure protocol overhead (no network latency)"
echo "========================================"
