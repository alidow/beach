/// <reference types="vite/client" />
/// <reference types="@testing-library/jest-dom" />

interface ImportMetaEnv {
  readonly VITE_SESSION_SERVER_URL?: string;
  readonly VITE_SECURE_SIGNALING?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
