#!/bin/bash
set -euo pipefail

# Compare SSH vs Beach latency results

echo "========================================"
echo "  SSH vs Beach Latency Comparison"
echo "========================================"
echo ""

if [[ ! -f /tmp/ssh-mean.txt ]] || [[ ! -f /tmp/beach-mean.txt ]]; then
  echo "ERROR: Run both benchmark scripts first:"
  echo "  ./scripts/benchmark-ssh-latency.sh"
  echo "  ./scripts/benchmark-beach-latency.sh"
  exit 1
fi

ssh_mean=$(cat /tmp/ssh-mean.txt)
ssh_p50=$(cat /tmp/ssh-p50.txt)
ssh_p95=$(cat /tmp/ssh-p95.txt)
ssh_p99=$(cat /tmp/ssh-p99.txt)

beach_mean=$(cat /tmp/beach-mean.txt)
beach_p50=$(cat /tmp/beach-p50.txt)
beach_p95=$(cat /tmp/beach-p95.txt)
beach_p99=$(cat /tmp/beach-p99.txt)

# Calculate differences
mean_diff=$((ssh_mean - beach_mean))
mean_pct=$(awk "BEGIN {printf \"%.1f\", ($mean_diff / $ssh_mean) * 100}")

p50_diff=$((ssh_p50 - beach_p50))
p50_pct=$(awk "BEGIN {printf \"%.1f\", ($p50_diff / $ssh_p50) * 100}")

p95_diff=$((ssh_p95 - beach_p95))
p95_pct=$(awk "BEGIN {printf \"%.1f\", ($p95_diff / $ssh_p95) * 100}")

p99_diff=$((ssh_p99 - beach_p99))
p99_pct=$(awk "BEGIN {printf \"%.1f\", ($p99_diff / $ssh_p99) * 100}")

# Display results
printf "%-12s %10s %10s %12s %12s\n" "Metric" "SSH" "Beach" "Difference" "% Faster"
printf "%-12s %10s %10s %12s %12s\n" "------" "---" "-----" "----------" "--------"
printf "%-12s %9dms %9dms %11dms %11s%%\n" "Mean" "$ssh_mean" "$beach_mean" "$mean_diff" "$mean_pct"
printf "%-12s %9dms %9dms %11dms %11s%%\n" "P50" "$ssh_p50" "$beach_p50" "$p50_diff" "$p50_pct"
printf "%-12s %9dms %9dms %11dms %11s%%\n" "P95" "$ssh_p95" "$beach_p95" "$p95_diff" "$p95_pct"
printf "%-12s %9dms %9dms %11dms %11s%%\n" "P99" "$ssh_p99" "$beach_p99" "$p99_diff" "$p99_pct"

echo ""
echo "========================================"

# Determine overall result
if (( mean_diff > 0 )); then
  echo "✓ Beach is FASTER than SSH by ${mean_pct}% (mean latency)"
else
  echo "✗ Beach is SLOWER than SSH by $(awk "BEGIN {printf \"%.1f\", -$mean_pct}")% (mean latency)"
fi

if [[ $(awk "BEGIN {print ($mean_pct >= 30)}") -eq 1 ]]; then
  echo "✓ Meets 30%+ performance goal!"
else
  echo "✗ Does not meet 30%+ performance goal"
fi

echo "========================================"
