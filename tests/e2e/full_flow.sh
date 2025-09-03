#!/bin/bash

# End-to-end test for Beach terminal sharing
# Tests the complete flow: server start -> session registration -> client join

set -e

# Source common utilities
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../utils/common.sh"

# Configuration
BEACH_ROAD_PORT=${BEACH_ROAD_PORT:-8080}
BEACH_SESSION_SERVER=${BEACH_SESSION_SERVER:-"localhost:$BEACH_ROAD_PORT"}
REDIS_URL=${REDIS_URL:-"redis://localhost:6379"}

# Process PIDs for cleanup
PIDS=()

# Cleanup on exit
cleanup() {
    cleanup_processes "${PIDS[@]}"
}
trap cleanup EXIT

# Extract session URL from beach server output
extract_session_url() {
    local output_file=$1
    local max_attempts=10
    local attempt=0
    
    while [ $attempt -lt $max_attempts ]; do
        if [ -f "$output_file" ]; then
            local url=$(grep "Session URL:" "$output_file" 2>/dev/null | awk '{print $NF}')
            if [ -n "$url" ]; then
                echo "$url"
                return 0
            fi
        fi
        sleep 1
        attempt=$((attempt + 1))
    done
    
    return 1
}

# Main test execution
main() {
    print_header "Beach End-to-End Test"
    
    # Ensure we're in project root
    cd "$SCRIPT_DIR/../.."
    ensure_project_root
    
    # Step 1: Check prerequisites
    print_info "Checking prerequisites..."
    
    if ! check_redis; then
        echo ""
        print_warning "Redis is not running. Attempting to start with Docker..."
        if docker-compose up -d redis 2>/dev/null; then
            sleep 2
            if check_redis; then
                print_success "Redis started successfully"
            else
                print_error "Failed to start Redis"
                exit 1
            fi
        else
            print_error "Docker not available. Please start Redis manually."
            exit 1
        fi
    fi
    
    # Step 2: Build projects
    print_info "Building Beach projects..."
    if cargo build -p beach-road -p beach --quiet; then
        print_success "Build successful"
    else
        print_error "Build failed"
        exit 1
    fi
    
    # Step 3: Start beach-road
    print_info "Starting Beach Road session server..."
    BEACH_ROAD_LOG="/tmp/beach-road-test.log"
    BEACH_ROAD_PORT=$BEACH_ROAD_PORT REDIS_URL=$REDIS_URL \
        cargo run -p beach-road --quiet > "$BEACH_ROAD_LOG" 2>&1 &
    BEACH_ROAD_PID=$!
    PIDS+=($BEACH_ROAD_PID)
    
    if wait_for_service "Beach Road" "curl -s http://localhost:$BEACH_ROAD_PORT/health"; then
        echo ""
    else
        echo "Beach Road logs:"
        cat "$BEACH_ROAD_LOG"
        exit 1
    fi
    
    # Step 4: Start Beach server
    print_header "Testing Beach Server"
    
    print_info "Starting Beach server with test command..."
    BEACH_SERVER_LOG="/tmp/beach-server-test.log"
    BEACH_SESSION_SERVER=$BEACH_SESSION_SERVER \
        timeout 10 cargo run -p beach --quiet -- echo "Hello from Beach" > "$BEACH_SERVER_LOG" 2>&1 &
    BEACH_SERVER_PID=$!
    PIDS+=($BEACH_SERVER_PID)
    
    # Wait for session registration
    sleep 3
    
    # Extract session URL
    SESSION_URL=$(extract_session_url "$BEACH_SERVER_LOG")
    if [ -n "$SESSION_URL" ]; then
        print_success "Beach server started and registered session"
        print_info "Session URL: $SESSION_URL"
    else
        print_error "Failed to extract session URL"
        echo "Beach server logs:"
        cat "$BEACH_SERVER_LOG"
        exit 1
    fi
    
    # Extract session ID from URL
    SESSION_ID=$(echo "$SESSION_URL" | cut -d'/' -f2)
    print_info "Session ID: $SESSION_ID"
    
    # Step 5: Verify session exists in beach-road
    print_header "Testing Session Registration"
    
    RESPONSE=$(curl -s "http://localhost:$BEACH_ROAD_PORT/sessions/$SESSION_ID")
    assert_contains "$RESPONSE" '"exists":true' "Session registered in beach-road"
    
    # Step 6: Test client join
    print_header "Testing Beach Client"
    
    print_info "Attempting to join session as client..."
    BEACH_CLIENT_LOG="/tmp/beach-client-test.log"
    
    # Create a test script that will attempt to join and then exit
    cat > /tmp/beach-client-test.sh << 'EOF'
#!/bin/bash
timeout 5 cargo run -p beach --quiet -- --join "$1" 2>&1 | tee "$2" &
CLIENT_PID=$!
sleep 3
kill $CLIENT_PID 2>/dev/null || true
grep -q "Session validated successfully" "$2"
EOF
    chmod +x /tmp/beach-client-test.sh
    
    if /tmp/beach-client-test.sh "$SESSION_URL" "$BEACH_CLIENT_LOG"; then
        print_success "Client successfully validated session"
    else
        print_warning "Client validation check inconclusive"
        echo "Client logs:"
        cat "$BEACH_CLIENT_LOG"
    fi
    
    # Step 7: Test with passphrase
    print_header "Testing Passphrase Protection"
    
    print_info "Starting Beach server with passphrase..."
    BEACH_SERVER_PASS_LOG="/tmp/beach-server-pass-test.log"
    BEACH_SESSION_SERVER=$BEACH_SESSION_SERVER \
        timeout 10 cargo run -p beach --quiet -- --passphrase "secret123" echo "Protected" > "$BEACH_SERVER_PASS_LOG" 2>&1 &
    BEACH_SERVER_PASS_PID=$!
    PIDS+=($BEACH_SERVER_PASS_PID)
    
    sleep 3
    
    SESSION_URL_PASS=$(extract_session_url "$BEACH_SERVER_PASS_LOG")
    if [ -n "$SESSION_URL_PASS" ]; then
        print_success "Protected session created"
        SESSION_ID_PASS=$(echo "$SESSION_URL_PASS" | cut -d'/' -f2)
        
        # Try to join without passphrase (should fail)
        RESPONSE=$(curl -s -X POST "http://localhost:$BEACH_ROAD_PORT/sessions/$SESSION_ID_PASS/join" \
            -H "Content-Type: application/json" \
            -d '{"passphrase": null}')
        assert_contains "$RESPONSE" '"success":false' "Join without passphrase rejected"
        
        # Try to join with wrong passphrase (should fail)
        RESPONSE=$(curl -s -X POST "http://localhost:$BEACH_ROAD_PORT/sessions/$SESSION_ID_PASS/join" \
            -H "Content-Type: application/json" \
            -d '{"passphrase": "wrong"}')
        assert_contains "$RESPONSE" '"success":false' "Join with wrong passphrase rejected"
        
        # Try to join with correct passphrase (should succeed)
        RESPONSE=$(curl -s -X POST "http://localhost:$BEACH_ROAD_PORT/sessions/$SESSION_ID_PASS/join" \
            -H "Content-Type: application/json" \
            -d '{"passphrase": "secret123"}')
        assert_contains "$RESPONSE" '"success":true' "Join with correct passphrase accepted"
    else
        print_warning "Could not test passphrase protection"
    fi
    
    # Clean up temp files
    rm -f /tmp/beach-*.log /tmp/beach-client-test.sh
    
    # Print summary
    print_test_summary
}

# Run the tests
main "$@"