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
  fallbackCohort: string;
  setFallbackCohort: (value: string) => void;
  fallbackEntitlement: string;
  setFallbackEntitlement: (value: string) => void;
  fallbackTelemetryOptIn: boolean;
  setFallbackTelemetryOptIn: (value: boolean) => void;
}

const FALLBACK_COHORT_KEY = 'beach:fallback:cohort';
const FALLBACK_ENTITLEMENT_KEY = 'beach:fallback:entitlement';
const FALLBACK_TELEMETRY_KEY = 'beach:fallback:telemetry';

function readStringSetting(key: string): string {
  if (typeof window === 'undefined') {
    return '';
  }
  try {
    return window.localStorage.getItem(key) ?? '';
  } catch {
    return '';
  }
}

function writeStringSetting(key: string, value: string): void {
  if (typeof window === 'undefined') {
    return;
  }
  try {
    if (value.trim().length > 0) {
      window.localStorage.setItem(key, value.trim());
    } else {
      window.localStorage.removeItem(key);
    }
  } catch {
    // ignore localStorage errors
  }
}

function readBooleanSetting(key: string): boolean {
  if (typeof window === 'undefined') {
    return false;
  }
  try {
    const raw = window.localStorage.getItem(key);
    if (!raw) {
      return false;
    }
    return ['1', 'true', 'yes', 'on'].includes(raw.trim().toLowerCase());
  } catch {
    return false;
  }
}

function writeBooleanSetting(key: string, value: boolean): void {
  if (typeof window === 'undefined') {
    return;
  }
  try {
    window.localStorage.setItem(key, value ? '1' : '0');
  } catch {
    // ignore localStorage errors
  }
}

function logFallbackSetting(event: string, payload: Record<string, unknown>): void {
  if (typeof console !== 'undefined') {
    console.info('[beach-surfer] fallback override updated', { event, ...payload });
  }
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
  const [fallbackCohort, setFallbackCohortState] = useState(() => readStringSetting(FALLBACK_COHORT_KEY));
  const [fallbackEntitlement, setFallbackEntitlementState] = useState(() => readStringSetting(FALLBACK_ENTITLEMENT_KEY));
  const [fallbackTelemetryOptIn, setFallbackTelemetryOptInState] = useState(() => readBooleanSetting(FALLBACK_TELEMETRY_KEY));

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

  const setFallbackCohort = useCallback((value: string) => {
    setFallbackCohortState(value);
    writeStringSetting(FALLBACK_COHORT_KEY, value);
    logFallbackSetting('cohort', { active: value.trim().length > 0 });
  }, []);

  const setFallbackEntitlement = useCallback((value: string) => {
    setFallbackEntitlementState(value);
    writeStringSetting(FALLBACK_ENTITLEMENT_KEY, value);
    logFallbackSetting('entitlement_proof', { provided: value.trim().length > 0 });
  }, []);

  const setFallbackTelemetryOptIn = useCallback((value: boolean) => {
    setFallbackTelemetryOptInState(value);
    writeBooleanSetting(FALLBACK_TELEMETRY_KEY, value);
    logFallbackSetting('telemetry_opt_in', { opt_in: value });
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
    fallbackCohort,
    setFallbackCohort,
    fallbackEntitlement,
    setFallbackEntitlement,
    fallbackTelemetryOptIn,
    setFallbackTelemetryOptIn,
  };
}
