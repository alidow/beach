import { SessionSummary } from '../lib/api';

type Props = {
  sessions: SessionSummary[];
  onSelect: (s: SessionSummary) => void;
};

export default function SessionTable({ sessions, onSelect }: Props) {
  return (
    <table style={{ width: '100%', borderCollapse: 'collapse', background: '#fff' }}>
      <thead>
        <tr>
          <th style={th}>Session</th>
          <th style={th}>Harness</th>
          <th style={th}>Location</th>
          <th style={th}>Queue</th>
          <th style={th}>Controller</th>
          <th style={th}>Health</th>
        </tr>
      </thead>
      <tbody>
        {sessions.map((s) => (
          <tr key={s.session_id} style={{ cursor: 'pointer' }} onClick={() => onSelect(s)}>
            <td style={tdMono}>{s.session_id.slice(0, 8)}</td>
            <td style={td}>{s.harness_type}</td>
            <td style={td}>{s.location_hint ?? '-'}</td>
            <td style={td}>{s.pending_actions} / {s.pending_unacked}</td>
            <td style={td}>{s.controller_token ? 'leased' : '—'}</td>
            <td style={td}>{s.last_health?.degraded ? 'degraded' : 'ok'}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

const th: React.CSSProperties = { textAlign: 'left', padding: '8px', borderBottom: '1px solid #eee', fontWeight: 600 };
const td: React.CSSProperties = { padding: '8px', borderBottom: '1px solid #f3f3f3' };
const tdMono: React.CSSProperties = { ...td, fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace' };
