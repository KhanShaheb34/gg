/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Base URL of gg-devserver for the browser build (default http://127.0.0.1:1421). */
  readonly VITE_GG_DEV_URL?: string;
}
