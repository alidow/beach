#!/bin/bash

# Common utilities for Beach test scripts

# Colors for output
export RED='\033[0;31m'
export GREEN='\033[0;32m'
export YELLOW='\033[1;33m'
export BLUE='\033[0;34m'
export NC='\033[0m' # No Color

# Test counters
export TESTS_PASSED=0
export TESTS_FAILED=0

# Print colored output
print_success() {
    echo -e "${GREEN}✓ $1${NC}"
}

print_error() {
    echo -e "${RED}✗ $1${NC}"
}

print_warning() {
    echo -e "${YELLOW}⚠ $1${NC}"
}

print_info() {
    echo -e "${BLUE}ℹ $1${NC}"
}

print_header() {
    echo ""
    echo "===================================="
    echo "$1"
    echo "===================================="
    echo ""
}

# Check if a service is running
check_service() {
    local service_name=$1
    local check_command=$2
    
    echo -n "Checking $service_name... "
    if eval "$check_command" > /dev/null 2>&1; then
        print_success "$service_name is running"
        return 0
    else
        print_error "$service_name is not running"
        return 1
    fi
}

# Check Redis availability
check_redis() {
    check_service "Redis" "redis-cli ping"
}

# Check if beach-road is running
check_beach_road() {
    local port=${1:-8080}
    check_service "Beach Road (port $port)" "curl -s http://localhost:$port/health"
}

# Start a background process with logging
start_background_process() {
    local name=$1
    local command=$2
    local log_prefix=$3
    
    echo "Starting $name..."
    eval "$command" 2>&1 | sed "s/^/  [$log_prefix] /" &
    local pid=$!
    
    # Store PID for cleanup
    echo $pid
}

# Wait for a service to be ready
wait_for_service() {
    local service_name=$1
    local check_command=$2
    local max_attempts=${3:-30}
    local attempt=0
    
    echo -n "Waiting for $service_name to be ready"
    while [ $attempt -lt $max_attempts ]; do
        if eval "$check_command" > /dev/null 2>&1; then
            echo ""
            print_success "$service_name is ready"
            return 0
        fi
        echo -n "."
        sleep 1
        attempt=$((attempt + 1))
    done
    
    echo ""
    print_error "$service_name failed to start (timeout after ${max_attempts}s)"
    return 1
}

# Cleanup function to kill background processes
cleanup_processes() {
    local pids=("$@")
    
    echo "Cleaning up processes..."
    for pid in "${pids[@]}"; do
        if [ -n "$pid" ] && kill -0 $pid 2>/dev/null; then
            kill $pid 2>/dev/null || true
            echo "  Stopped process $pid"
        fi
    done
}

# Test assertion functions
assert_equals() {
    local expected=$1
    local actual=$2
    local test_name=$3
    
    if [ "$expected" = "$actual" ]; then
        print_success "$test_name"
        TESTS_PASSED=$((TESTS_PASSED + 1))
        return 0
    else
        print_error "$test_name (expected: '$expected', got: '$actual')"
        TESTS_FAILED=$((TESTS_FAILED + 1))
        return 1
    fi
}

assert_contains() {
    local haystack=$1
    local needle=$2
    local test_name=$3
    
    if echo "$haystack" | grep -q "$needle"; then
        print_success "$test_name"
        TESTS_PASSED=$((TESTS_PASSED + 1))
        return 0
    else
        print_error "$test_name (string not found: '$needle')"
        TESTS_FAILED=$((TESTS_FAILED + 1))
        return 1
    fi
}

assert_http_status() {
    local url=$1
    local expected_status=$2
    local test_name=$3
    
    local actual_status=$(curl -s -o /dev/null -w "%{http_code}" "$url")
    assert_equals "$expected_status" "$actual_status" "$test_name"
}

# Print test summary
print_test_summary() {
    echo ""
    print_header "Test Summary"
    
    local total=$((TESTS_PASSED + TESTS_FAILED))
    echo "Total tests: $total"
    
    if [ $TESTS_PASSED -gt 0 ]; then
        print_success "Passed: $TESTS_PASSED"
    fi
    
    if [ $TESTS_FAILED -gt 0 ]; then
        print_error "Failed: $TESTS_FAILED"
    fi
    
    echo ""
    
    if [ $TESTS_FAILED -eq 0 ]; then
        print_success "All tests passed! 🎉"
        return 0
    else
        print_error "Some tests failed"
        return 1
    fi
}

# Ensure we're in the project root
ensure_project_root() {
    if [ ! -f "Cargo.toml" ] || [ ! -d "apps/beach" ]; then
        print_error "This script must be run from the Beach project root"
        exit 1
    fi
}

# Export functions so they're available to sourcing scripts
export -f print_success print_error print_warning print_info print_header
export -f check_service check_redis check_beach_road
export -f start_background_process wait_for_service cleanup_processes
export -f assert_equals assert_contains assert_http_status
export -f print_test_summary ensure_project_root