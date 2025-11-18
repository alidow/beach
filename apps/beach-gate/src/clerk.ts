import { randomUUID, randomBytes } from 'node:crypto';
import { BeachGateConfig } from './config.js';

export interface DeviceStartRequest {
  scope?: string;
  audience?: string;
  userAgent?: string;
}

export interface DeviceStartResponse {
  deviceCode: string;
  userCode: string;
  verificationUri: string;
  verificationUriComplete: string;
  expiresIn: number;
  interval: number;
}

export interface DeviceFinishResult {
  userId: string;
  email?: string;
  clerkRefreshToken?: string;
  clerkAccessToken: string;
  clerkIdToken?: string;
}

export interface ClerkUserInfo {
  sub: string;
  email?: string;
  email_verified?: boolean;
  [key: string]: unknown;
}

export interface ClerkClient {
  startDeviceAuthorization(request: DeviceStartRequest): Promise<DeviceStartResponse>;
  finishDeviceAuthorization(deviceCode: string): Promise<DeviceFinishResult>;
  getUserInfo(accessToken: string): Promise<ClerkUserInfo>;
}

const DEVICE_PATH = '/oauth/device_authorization';
const TOKEN_PATH = '/oauth/token';
const USERINFO_PATH = '/oauth/userinfo';
const MOCK_USER_ID = '00000000-0000-0000-0000-000000000001';

export function createClerkClient(config: BeachGateConfig): ClerkClient {
  if (config.mockClerk) {
    return new MockClerkClient(config);
  }

  if (!config.clerkIssuerUrl || !config.clerkClientId || !config.clerkClientSecret) {
    throw new Error('Clerk configuration missing. Provide CLERK_ISSUER_URL, CLERK_CLIENT_ID, and CLERK_CLIENT_SECRET.');
  }

  return new ClerkRestClient(config);
}

class ClerkRestClient implements ClerkClient {
  constructor(private readonly config: BeachGateConfig) {}

  async startDeviceAuthorization(request: DeviceStartRequest): Promise<DeviceStartResponse> {
    const url = new URL(DEVICE_PATH, this.config.clerkIssuerUrl);
    const scope = request.scope ?? 'openid email offline_access';
    const body = new URLSearchParams({
      client_id: this.config.clerkClientId!,
      scope,
    });

    const audience = request.audience ?? this.config.clerkAudience;
    if (audience) {
      body.set('audience', audience);
    }

    const response = await fetch(url, {
      method: 'POST',
      headers: {
        'content-type': 'application/x-www-form-urlencoded',
      },
      body,
    });

    if (!response.ok) {
      const text = await response.text();
      throw new Error(`Clerk device authorization failed (${response.status}): ${text}`);
    }

    const json = (await response.json()) as Record<string, unknown>;
    return mapDeviceResponse(json);
  }

  async finishDeviceAuthorization(deviceCode: string): Promise<DeviceFinishResult> {
    const url = new URL(TOKEN_PATH, this.config.clerkIssuerUrl);
    const body = new URLSearchParams({
      grant_type: 'urn:ietf:params:oauth:grant-type:device_code',
      device_code: deviceCode,
      client_id: this.config.clerkClientId!,
      client_secret: this.config.clerkClientSecret!,
    });

    const response = await fetch(url, {
      method: 'POST',
      headers: {
        'content-type': 'application/x-www-form-urlencoded',
      },
      body,
    });

    if (!response.ok) {
      const text = await response.text();
      throw new Error(`Clerk device token exchange failed (${response.status}): ${text}`);
    }

    const json = (await response.json()) as Record<string, unknown>;
    const accessToken = String(json.access_token ?? '');
    const idToken = json.id_token ? String(json.id_token) : undefined;
    const refreshToken = json.refresh_token ? String(json.refresh_token) : undefined;

    if (!accessToken) {
      throw new Error('Clerk response missing access_token.');
    }

    const userInfo = await this.getUserInfo(accessToken);

    return {
      userId: userInfo.sub,
      email: typeof userInfo.email === 'string' ? userInfo.email : undefined,
      clerkAccessToken: accessToken,
      clerkRefreshToken: refreshToken,
      clerkIdToken: idToken,
    };
  }

  async getUserInfo(accessToken: string): Promise<ClerkUserInfo> {
    const url = new URL(USERINFO_PATH, this.config.clerkIssuerUrl);
    const response = await fetch(url, {
      headers: {
        authorization: `Bearer ${accessToken}`,
      },
    });

    if (!response.ok) {
      const text = await response.text();
      throw new Error(`Failed to fetch Clerk user info (${response.status}): ${text}`);
    }

    const json = (await response.json()) as ClerkUserInfo;
    return json;
  }
}

type MockDeviceRecord = {
  deviceCode: string;
  userCode: string;
  userId: string;
  email: string;
  expiresAt: number;
};

class MockClerkClient implements ClerkClient {
  private readonly devices = new Map<string, MockDeviceRecord>();

  constructor(_config: BeachGateConfig) {
    // beach-gate mock mode does not require real Clerk config
  }

  async startDeviceAuthorization(_request: DeviceStartRequest): Promise<DeviceStartResponse> {
    const deviceCode = randomUUID();
    const userCode = randomBytes(3).toString('hex').toUpperCase();
    const record: MockDeviceRecord = {
      deviceCode,
      userCode,
      userId: MOCK_USER_ID,
      email: 'mock-user@beach.test',
      expiresAt: Date.now() + 5 * 60 * 1000,
    };

    this.devices.set(deviceCode, record);

    return {
      deviceCode,
      userCode,
      verificationUri: 'https://mock.clerk.dev/verify',
      verificationUriComplete: `https://mock.clerk.dev/verify?code=${userCode}`,
      expiresIn: 300,
      interval: 5,
    };
  }

  async finishDeviceAuthorization(deviceCode: string): Promise<DeviceFinishResult> {
    const record = this.devices.get(deviceCode);
    if (!record || record.expiresAt < Date.now()) {
      throw new Error('Invalid or expired device code');
    }

    this.devices.delete(deviceCode);

    return {
      userId: record.userId,
      email: record.email,
      clerkAccessToken: randomBytes(16).toString('hex'),
      clerkRefreshToken: randomBytes(16).toString('hex'),
    };
  }

  async getUserInfo(_accessToken: string): Promise<ClerkUserInfo> {
    return {
      sub: 'mock-user',
      email: 'mock-user@beach.test',
      email_verified: true,
    };
  }
}

function mapDeviceResponse(json: Record<string, unknown>): DeviceStartResponse {
  const required = ['device_code', 'user_code', 'verification_uri'];
  for (const key of required) {
    if (!json[key]) {
      throw new Error(`Clerk device response missing ${key}`);
    }
  }

  return {
    deviceCode: String(json.device_code),
    userCode: String(json.user_code),
    verificationUri: String(json.verification_uri),
    verificationUriComplete: String(json.verification_uri_complete ?? json.verification_uri),
    expiresIn: Number(json.expires_in ?? 300),
    interval: Number(json.interval ?? 5),
  };
}
