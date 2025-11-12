import { TerminalGridStore } from '../../../beach-surfer/src/terminal/gridStore';
import type { TerminalTransport } from '../../../beach-surfer/src/transport/terminalTransport';
import type { SecureTransportSummary } from '../../../beach-surfer/src/transport/webrtc';

export type TerminalViewerStatus = 'idle' | 'connecting' | 'connected' | 'reconnecting' | 'error';

export type TerminalViewerState = {
  store: TerminalGridStore | null;
  transport: TerminalTransport | null;
  transportVersion?: number;
  connecting: boolean;
  error: string | null;
  status: TerminalViewerStatus;
  secureSummary: SecureTransportSummary | null;
  latencyMs: number | null;
};

export type SessionCredentialOverride = {
  passcode?: string | null;
  viewerToken?: string | null;
  authorizationToken?: string | null;
  skipCredentialFetch?: boolean;
};
