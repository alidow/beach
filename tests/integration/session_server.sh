#!/bin/bash

# Integration test for Beach session server (beach-road)
# Tests session registration, validation, and client joining

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

# Main test execution
main() {
    print_header "Beach Session Server Integration Test"
    
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
                echo "Please start Redis manually:"
                echo "  - Docker: docker-compose up -d redis"
                echo "  - Local: redis-server"
                exit 1
            fi
        else
            print_error "Docker is not available or docker-compose failed"
            echo "Please start Redis manually:"
            echo "  - Docker: docker-compose up -d redis"
            echo "  - Local: redis-server"
            exit 1
        fi
    fi
    
    # Step 2: Build the projects
    print_info "Building Beach projects..."
    if cargo build -p beach-road -p beach --quiet; then
        print_success "Build successful"
    else
        print_error "Build failed"
        exit 1
    fi
    
    # Step 3: Start beach-road
    print_info "Starting Beach Road session server..."
    BEACH_ROAD_PID=$(start_background_process \
        "Beach Road" \
        "BEACH_ROAD_PORT=$BEACH_ROAD_PORT REDIS_URL=$REDIS_URL cargo run -p beach-road --quiet" \
        "beach-road")
    PIDS+=($BEACH_ROAD_PID)
    
    # Wait for beach-road to be ready
    if wait_for_service "Beach Road" "curl -s http://localhost:$BEACH_ROAD_PORT/health"; then
        echo ""
    else
        exit 1
    fi
    
    # Step 4: Test health endpoint
    print_header "Testing Beach Road Endpoints"
    
    assert_http_status \
        "http://localhost:$BEACH_ROAD_PORT/health" \
        "200" \
        "Health check endpoint"
    
    # Step 5: Test session registration
    print_info "Testing session registration..."
    
    RESPONSE=$(curl -s -X POST "http://localhost:$BEACH_ROAD_PORT/sessions" \
        -H "Content-Type: application/json" \
        -d '{
            "session_id": "test-session-001",
            "passphrase": "test-pass"
        }')
    
    assert_contains "$RESPONSE" '"success":true' "Session registration successful"
    assert_contains "$RESPONSE" "test-session-001" "Session URL contains session ID"
    
    # Step 6: Test duplicate session registration
    print_info "Testing duplicate session prevention..."
    
    RESPONSE=$(curl -s -X POST "http://localhost:$BEACH_ROAD_PORT/sessions" \
        -H "Content-Type: application/json" \
        -d '{
            "session_id": "test-session-001",
            "passphrase": "test-pass"
        }')
    
    assert_contains "$RESPONSE" '"success":false' "Duplicate session rejected"
    assert_contains "$RESPONSE" "already exists" "Error message indicates duplicate"
    
    # Step 7: Test session status check
    print_info "Testing session status check..."
    
    RESPONSE=$(curl -s "http://localhost:$BEACH_ROAD_PORT/sessions/test-session-001")
    assert_contains "$RESPONSE" '"exists":true' "Session exists check"
    
    RESPONSE=$(curl -s "http://localhost:$BEACH_ROAD_PORT/sessions/nonexistent-session")
    assert_contains "$RESPONSE" '"exists":false' "Nonexistent session check"
    
    # Step 8: Test joining session with correct passphrase
    print_info "Testing session join with correct passphrase..."
    
    RESPONSE=$(curl -s -X POST "http://localhost:$BEACH_ROAD_PORT/sessions/test-session-001/join" \
        -H "Content-Type: application/json" \
        -d '{
            "passphrase": "test-pass"
        }')
    
    assert_contains "$RESPONSE" '"success":true' "Join with correct passphrase"
    assert_contains "$RESPONSE" "webrtc_offer" "WebRTC placeholder present"
    
    # Step 9: Test joining session with wrong passphrase
    print_info "Testing session join with wrong passphrase..."
    
    RESPONSE=$(curl -s -X POST "http://localhost:$BEACH_ROAD_PORT/sessions/test-session-001/join" \
        -H "Content-Type: application/json" \
        -d '{
            "passphrase": "wrong-pass"
        }')
    
    assert_contains "$RESPONSE" '"success":false' "Join with wrong passphrase rejected"
    assert_contains "$RESPONSE" "Invalid passphrase" "Error message indicates wrong passphrase"
    
    # Step 10: Test joining nonexistent session
    print_info "Testing join nonexistent session..."
    
    RESPONSE=$(curl -s -X POST "http://localhost:$BEACH_ROAD_PORT/sessions/nonexistent/join" \
        -H "Content-Type: application/json" \
        -d '{
            "passphrase": "any-pass"
        }')
    
    assert_contains "$RESPONSE" '"success":false' "Join nonexistent session rejected"
    assert_contains "$RESPONSE" "not found" "Error message indicates session not found"
    
    # Step 11: Test session without passphrase
    print_info "Testing session without passphrase..."
    
    RESPONSE=$(curl -s -X POST "http://localhost:$BEACH_ROAD_PORT/sessions" \
        -H "Content-Type: application/json" \
        -d '{
            "session_id": "test-session-002",
            "passphrase": null
        }')
    
    assert_contains "$RESPONSE" '"success":true' "Session without passphrase created"
    
    RESPONSE=$(curl -s -X POST "http://localhost:$BEACH_ROAD_PORT/sessions/test-session-002/join" \
        -H "Content-Type: application/json" \
        -d '{
            "passphrase": null
        }')
    
    assert_contains "$RESPONSE" '"success":true' "Join session without passphrase"
    
    # Print summary
    print_test_summary
}

# Run the tests
main "$@"