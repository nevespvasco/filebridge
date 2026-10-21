/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** `1` runs the UI in a plain browser against an in-memory fake service (src/dev/mock.ts). */
  readonly VITE_MOCK?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
