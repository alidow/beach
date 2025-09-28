import { useState } from 'react';
import { BeachTerminal } from './components/BeachTerminal';

export default function App(): JSX.Element {
  const [sessionId, setSessionId] = useState('');
  const [baseUrl, setBaseUrl] = useState('http://127.0.0.1:8080');
  const [passcode, setPasscode] = useState('');
  const [autoConnect, setAutoConnect] = useState(false);

  return (
    <main style={styles.shell}>
      <section style={styles.panel}>
        <h1 style={styles.heading}>Beach Web</h1>
        <p style={styles.text}>Experimental React/WebRTC terminal client.</p>
        <div style={styles.formGrid}>
          <label style={styles.label}>
            Session ID
            <input
              style={styles.input}
              value={sessionId}
              onChange={(event) => setSessionId(event.target.value)}
              placeholder="00000000-0000-0000-0000-000000000000"
            />
          </label>
          <label style={styles.label}>
            Base URL
            <input
              style={styles.input}
              value={baseUrl}
              onChange={(event) => setBaseUrl(event.target.value)}
              placeholder="http://127.0.0.1:8080"
            />
          </label>
          <label style={styles.label}>
            Passcode
            <input
              style={styles.input}
              value={passcode}
              onChange={(event) => setPasscode(event.target.value)}
              placeholder="optional"
            />
          </label>
        </div>
        <label style={styles.checkboxRow}>
          <input
            type="checkbox"
            checked={autoConnect}
            onChange={(event) => setAutoConnect(event.target.checked)}
          />
          Auto connect
        </label>
        <div style={styles.terminalShell}>
          <BeachTerminal
            sessionId={sessionId || undefined}
            baseUrl={baseUrl || undefined}
            passcode={passcode || undefined}
            autoConnect={autoConnect}
            style={{ flex: 1, minHeight: 0 }}
          />
        </div>
      </section>
    </main>
  );
}

const styles: Record<string, React.CSSProperties> = {
  shell: {
    minHeight: '100vh',
    margin: 0,
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    background: '#0b1021',
    color: '#f8fafc',
    fontFamily: "'SF Pro Text', 'Inter', system-ui, -apple-system, BlinkMacSystemFont, sans-serif",
    padding: '3rem 1.5rem',
  },
  panel: {
    width: 'min(720px, 95vw)',
    minHeight: '70vh',
    background: '#11162b',
    borderRadius: 16,
    padding: '2.5rem',
    boxShadow: '0 18px 48px rgba(15, 23, 42, 0.45)',
    display: 'flex',
    flexDirection: 'column',
    gap: '1.5rem',
  },
  heading: {
    fontSize: '1.75rem',
    marginBottom: '0.5rem',
  },
  text: {
    margin: 0,
    opacity: 0.8,
  },
  formGrid: {
    display: 'grid',
    gap: '1rem',
  },
  label: {
    display: 'flex',
    flexDirection: 'column',
    gap: '0.5rem',
    fontSize: '0.95rem',
    letterSpacing: 0.05,
    textTransform: 'uppercase',
    opacity: 0.8,
  },
  checkboxRow: {
    display: 'flex',
    alignItems: 'center',
    gap: '0.75rem',
    letterSpacing: 0.05,
    textTransform: 'uppercase',
    opacity: 0.8,
  },
  input: {
    padding: '0.75rem 1rem',
    borderRadius: 10,
    border: '1px solid rgba(148, 163, 184, 0.4)',
    background: 'rgba(15, 23, 42, 0.4)',
    color: '#f8fafc',
    fontSize: '1rem',
    outline: 'none',
  },
  terminalShell: {
    flex: 1,
    minHeight: 280,
    background: '#020617',
    borderRadius: 12,
    border: '1px solid rgba(148, 163, 184, 0.25)',
    padding: 8,
    display: 'flex',
    flexDirection: 'column',
  },
};
