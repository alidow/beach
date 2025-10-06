
#!/usr/bin/env python3
"""Analyze predictive trace logs for the Beach clients.

This script consumes one or more log files that contain JSON events emitted by the
predictive logging instrumentation in the Rust TUI and web clients. It reconstructs
per-sequence timelines and surfaces potential issues such as missing acknowledgements,
server updates that did not match predictions, or predictions that never cleared.
"""
from __future__ import annotations

import argparse
import json
import math
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Tuple


@dataclass
class PredictionEvent:
    raw: Dict[str, Any]
    time: float
    event: str

    @property
    def seq(self) -> Optional[int]:
        value = self.raw.get('seq')
        if isinstance(value, (int, float)):
            return int(value)
        return None


@dataclass
class SequenceState:
    registered: Optional[PredictionEvent] = None
    overlaps: List[Tuple[PredictionEvent, Dict[str, Any]]] = field(default_factory=list)
    acks: List[PredictionEvent] = field(default_factory=list)
    clears: List[PredictionEvent] = field(default_factory=list)

    def register(self, event: PredictionEvent) -> None:
        self.registered = event

    def add_overlap(self, event: PredictionEvent, hit: Dict[str, Any]) -> None:
        self.overlaps.append((event, hit))

    def add_ack(self, event: PredictionEvent) -> None:
        self.acks.append(event)

    def add_clear(self, event: PredictionEvent) -> None:
        self.clears.append(event)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('logs', nargs='+', help='Log file(s) containing predictive events')
    parser.add_argument('--verbose', '-v', action='store_true', help='Print per-sequence timelines')
    parser.add_argument('--fail-on-issues', action='store_true', help='Exit with non-zero status if issues are found')
    return parser.parse_args()


def extract_json(line: str) -> Optional[Dict[str, Any]]:
    start = line.find('{')
    if start == -1:
        return None
    substring = line[start:]
    try:
        data = json.loads(substring)
    except json.JSONDecodeError:
        # Some lines may embed JSON as key=value pairs (e.g., payload={...}). Try to locate
        # the substring bounded by balanced braces.
        depth = 0
        for idx, ch in enumerate(substring):
            if ch == '{':
                depth += 1
            elif ch == '}':
                depth -= 1
                if depth == 0:
                    try:
                        return json.loads(substring[: idx + 1])
                    except json.JSONDecodeError:
                        return None
        return None
    else:
        return data if isinstance(data, dict) else None


def parse_events(paths: Iterable[Path]) -> List[Dict[str, Any]]:
    events: List[Dict[str, Any]] = []
    for path in paths:
        try:
            with path.open() as handle:
                for line in handle:
                    record = extract_json(line.strip())
                    if record:
                        events.append(record)
        except OSError as exc:
            print(f"warning: unable to read {path}: {exc}", file=sys.stderr)
    return events


def event_time(record: Dict[str, Any]) -> float:
    value = record.get('elapsed_ms')
    if isinstance(value, (int, float)):
        return float(value)
    # Some web events may omit elapsed_ms; fall back to timestamp if available.
    value = record.get('timestamp_ms')
    if isinstance(value, (int, float)):
        return float(value)
    return math.nan


def build_prediction_event(record: Dict[str, Any]) -> PredictionEvent:
    return PredictionEvent(raw=record, time=event_time(record), event=record.get('event', 'unknown'))


def analyse_source(source: str, records: List[Dict[str, Any]], verbose: bool) -> Tuple[List[str], Dict[int, SequenceState]]:
    events = [build_prediction_event(rec) for rec in records]
    events.sort(key=lambda e: (math.isnan(e.time), e.time))
    sequences: Dict[int, SequenceState] = {}
    issues: List[str] = []

    def state_for(seq: int) -> SequenceState:
        return sequences.setdefault(seq, SequenceState())

    for event in events:
        kind = event.event
        if kind == 'prediction_registered':
            seq = event.seq
            if seq is None:
                continue
            state_for(seq).register(event)
        elif kind == 'prediction_update_overlap':
            hits = event.raw.get('hits', [])
            if not isinstance(hits, list):
                continue
            for hit in hits:
                seq = hit.get('seq')
                if isinstance(seq, (int, float)):
                    state_for(int(seq)).add_overlap(event, hit)
        elif kind == 'prediction_ack':
            seq = event.seq
            if seq is None:
                continue
            state_for(seq).add_ack(event)
        elif kind == 'prediction_cleared':
            seq = event.seq
            if seq is None:
                continue
            state_for(seq).add_clear(event)

    # Evaluate sequence states
    for seq, state in sequences.items():
        if state.registered is None:
            issues.append(f"seq {seq} acknowledged/cleared without a registration")
            continue
        if not state.overlaps:
            issues.append(f"seq {seq} had no server overlap events")
        else:
            mismatches = [hit for _, hit in state.overlaps if not hit.get('match', False) and not hit.get('trimmed')]
            if mismatches:
                locs = ', '.join(f"(row={hit.get('row')}, col={hit.get('col')})" for hit in mismatches[:5])
                issues.append(f"seq {seq} server content did not match predictions at {locs}")
        if not state.acks:
            issues.append(f"seq {seq} never received an input_ack")
        else:
            if all(not ack.raw.get('cleared', False) and not state.clears for ack in state.acks):
                issues.append(f"seq {seq} acked but predictions never cleared")
        if verbose:
            print(f"    seq {seq}")
            if state.registered:
                print(f"      registered at {state.registered.time:.3f} ms")
            for overlap_event, hit in state.overlaps:
                status = 'match' if hit.get('match') else 'mismatch'
                if hit.get('trimmed'):
                    status = 'trimmed'
                print(
                    f"      overlap at {overlap_event.time:.3f} ms -> seq={hit.get('seq')} row={hit.get('row')} col={hit.get('col')} {status}"
                )
            for ack in state.acks:
                delay = ack.raw.get('ack_delay_ms')
                cleared = ack.raw.get('cleared')
                print(
                    f"      ack at {ack.time:.3f} ms delay={delay} cleared={cleared}"
                )
            for clear in state.clears:
                print(
                    f"      cleared ({clear.raw.get('reason')}) at {clear.time:.3f} ms"
                )
    return issues, sequences


def main() -> None:
    args = parse_args()
    paths = [Path(p) for p in args.logs]
    raw_events = parse_events(paths)
    if not raw_events:
        print('no predictive events found', file=sys.stderr)
        return

    by_source: Dict[str, List[Dict[str, Any]]] = {}
    for record in raw_events:
        source = record.get('source', 'unknown')
        by_source.setdefault(str(source), []).append(record)

    overall_issues: List[str] = []
    for source, records in sorted(by_source.items()):
        print(f"Source: {source} ({len(records)} events)")
        issues, sequences = analyse_source(source, records, verbose=args.verbose)
        if sequences:
            print(f"  sequences analysed: {len(sequences)}")
        else:
            print("  no sequences observed")
        if issues:
            overall_issues.extend(f"{source}: {issue}" for issue in issues)
            for issue in issues:
                print(f"  ! {issue}")
        else:
            print("  no issues detected")
        print()

    if overall_issues:
        print('Summary of issues:')
        for issue in overall_issues:
            print(f"  - {issue}")
        if args.fail_on_issues:
            sys.exit(1)
    else:
        print('No predictive inconsistencies detected.')


if __name__ == '__main__':
    main()
