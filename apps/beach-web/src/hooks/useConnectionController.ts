import { useCallback, useEffect, useMemo, useState } from 'react';
import type { TerminalStatus } from '../components/BeachTerminal';

interface ConnectionOptions {
  defaultServer?: string;
}

interface ConnectionController {
  sessionId: string;
  setSessionId: (value: string) => void;
  sessionServer: string;
  setSessionServer: (value: string) => void;
  passcode: string;
  setPasscode: (value: string) => void;
  status: TerminalStatus;
  setStatusDirect: (status: TerminalStatus) => void;
  connectRequested: boolean;
  requestConnect: () => void;
  cancelConnect: () => void;
  trimmedSessionId: string;
  trimmedServer: string;
  isConnecting: boolean;
  connectDisabled: boolean;
  connectLabel: string;
  onStatusChange: (status: TerminalStatus) => void;
}

export function useConnectionController(options: ConnectionOptions = {}): ConnectionController {
  const defaultServer = useMemo(
    () => options.defaultServer ?? import.meta.env.VITE_SESSION_SERVER_URL ?? 'https://api.beach.sh',
    [options.defaultServer],
  );
  const [sessionId, setSessionId] = useState('');
  const [sessionServer, setSessionServer] = useState(defaultServer);
  const [passcode, setPasscode] = useState('');
  const [status, setStatus] = useState<TerminalStatus>('idle');
  const [connectRequested, setConnectRequested] = useState(false);

  const trimmedSessionId = useMemo(() => sessionId.trim(), [sessionId]);
  const trimmedServer = useMemo(() => sessionServer.trim(), [sessionServer]);
  const isConnecting = status === 'connecting';
  const connectDisabled = useMemo(
    () => !trimmedSessionId || !trimmedServer || isConnecting,
    [trimmedSessionId, trimmedServer, isConnecting],
  );

  useEffect(() => {
    if (status === 'error' || status === 'closed') {
      setConnectRequested(false);
    }
  }, [status]);

  const requestConnect = useCallback(() => {
    if (connectDisabled) {
      return;
    }
    setConnectRequested(true);
  }, [connectDisabled]);

  const cancelConnect = useCallback(() => {
    setConnectRequested(false);
  }, []);

  const onStatusChange = useCallback((nextStatus: TerminalStatus) => {
    setStatus(nextStatus);
  }, []);

  const connectLabel = useMemo(() => {
    if (isConnecting) {
      return 'Connecting…';
    }
    if (status === 'connected') {
      return 'Reconnect';
    }
    return 'Connect';
  }, [isConnecting, status]);

  return {
    sessionId,
    setSessionId,
    sessionServer,
    setSessionServer,
    passcode,
    setPasscode,
    status,
    setStatusDirect: setStatus,
    connectRequested,
    requestConnect,
    cancelConnect,
    trimmedSessionId,
    trimmedServer,
    isConnecting,
    connectDisabled,
    connectLabel,
    onStatusChange,
  };
}
