#!/usr/bin/env python3
"""
Analyze beach debug recordings to reconstruct and compare states.

Usage:
    python3 analyze-debug-recording.py <debug-recording.jsonl>
    python3 analyze-debug-recording.py --compare <client.jsonl> <server.jsonl>
"""

import json
import sys
from datetime import datetime
from typing import Dict, List, Any, Optional
from collections import defaultdict

class DebugAnalyzer:
    def __init__(self):
        self.events = []
        self.client_states = []
        self.server_states = []
        self.messages = defaultdict(list)
        
    def load_file(self, filepath: str):
        """Load a JSONL debug recording file."""
        with open(filepath, 'r') as f:
            for line in f:
                if line.strip():
                    try:
                        event = json.loads(line)
                        self.events.append(event)
                    except json.JSONDecodeError as e:
                        print(f"Error parsing line: {e}")
                        print(f"Line: {line[:100]}...")
    
    def analyze(self):
        """Analyze loaded events and categorize them."""
        for event in self.events:
            event_type = event.get('type')
            
            if event_type == 'client_message':
                self.messages['client'].append(event)
            elif event_type == 'server_message':
                self.messages['server'].append(event)
            elif event_type == 'client_grid_state':
                self.client_states.append(event)
            elif event_type == 'server_backend_state':
                self.server_states.append(event)
            elif event_type == 'server_subscription_view':
                self.messages['subscription_view'].append(event)
    
    def print_summary(self):
        """Print a summary of the analyzed events."""
        print(f"\n📊 Debug Recording Summary")
        print(f"{'='*50}")
        print(f"Total events: {len(self.events)}")
        print(f"Client messages: {len(self.messages['client'])}")
        print(f"Server messages: {len(self.messages['server'])}")
        print(f"Client grid states: {len(self.client_states)}")
        print(f"Server backend states: {len(self.server_states)}")
        print(f"Subscription views: {len(self.messages['subscription_view'])}")
    
    def print_timeline(self, limit: int = 20):
        """Print a timeline of events."""
        print(f"\n📅 Event Timeline (showing first {limit} events)")
        print(f"{'='*50}")
        
        for i, event in enumerate(self.events[:limit]):
            timestamp = event.get('timestamp', 'N/A')
            event_type = event.get('type', 'unknown')
            
            # Format based on event type
            if event_type == 'client_message':
                msg = event.get('message', {})
                msg_type = list(msg.keys())[0] if msg else 'unknown'
                print(f"{i:3d}. [{timestamp}] CLIENT → {msg_type}")
                
            elif event_type == 'server_message':
                msg = event.get('message', {})
                msg_type = list(msg.keys())[0] if msg else 'unknown'
                print(f"{i:3d}. [{timestamp}] SERVER → {msg_type}")
                
            elif event_type == 'client_grid_state':
                grid = event.get('grid', {})
                w = grid.get('width', 0)
                h = grid.get('height', 0)
                mode = event.get('view_mode', 'unknown')
                print(f"{i:3d}. [{timestamp}] CLIENT_GRID: {w}x{h} ({mode})")
                
            elif event_type == 'server_backend_state':
                grid = event.get('grid', {})
                w = grid.get('width', 0)
                h = grid.get('height', 0)
                print(f"{i:3d}. [{timestamp}] SERVER_GRID: {w}x{h}")
    
    def find_discrepancies(self):
        """Find potential discrepancies between client and server states."""
        print(f"\n🔍 Checking for State Discrepancies")
        print(f"{'='*50}")
        
        # Check for blank lines in client grid states
        blank_line_events = []
        for state in self.client_states:
            grid = state.get('grid', {})
            cells = grid.get('cells', [])
            
            # Check for completely blank rows
            for row_idx, row in enumerate(cells):
                if row and all(cell.get('content', ' ') == ' ' for cell in row):
                    blank_line_events.append({
                        'timestamp': state.get('timestamp'),
                        'row': row_idx,
                        'grid_size': f"{grid.get('width')}x{grid.get('height')}"
                    })
        
        if blank_line_events:
            print(f"Found {len(blank_line_events)} events with blank lines:")
            for event in blank_line_events[:5]:
                print(f"  - [{event['timestamp']}] Row {event['row']} in {event['grid_size']} grid")
        else:
            print("No blank line issues detected in client states")
        
        # Check for snapshot/delta sequence issues
        print(f"\n📦 Message Sequence Analysis")
        snapshot_count = 0
        delta_count = 0
        
        for msg_event in self.messages['server']:
            msg = msg_event.get('message', {})
            if 'Snapshot' in msg:
                snapshot_count += 1
            elif 'Delta' in msg:
                delta_count += 1
        
        print(f"Snapshots received: {snapshot_count}")
        print(f"Deltas received: {delta_count}")
        
        if delta_count > 0:
            print(f"Delta/Snapshot ratio: {delta_count/max(1, snapshot_count):.2f}")
    
    def compare_states_at_time(self, timestamp: str):
        """Compare client and server states at a specific timestamp."""
        # Find closest client and server states to the timestamp
        # This would require timestamp parsing and comparison
        pass

def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)
    
    analyzer = DebugAnalyzer()
    
    if sys.argv[1] == '--compare' and len(sys.argv) == 4:
        print(f"Loading client recording: {sys.argv[2]}")
        analyzer.load_file(sys.argv[2])
        client_events = len(analyzer.events)
        
        print(f"Loading server recording: {sys.argv[3]}")
        analyzer.load_file(sys.argv[3])
        print(f"Client events: {client_events}, Server events: {len(analyzer.events) - client_events}")
    else:
        print(f"Loading recording: {sys.argv[1]}")
        analyzer.load_file(sys.argv[1])
    
    analyzer.analyze()
    analyzer.print_summary()
    analyzer.print_timeline()
    analyzer.find_discrepancies()

if __name__ == "__main__":
    main()