// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { HttpAuth } from './http-auth';
import type { StaticHeader } from './static-header';

/**
 * The `tool-rest` bucket: an HTTP endpoint.
 */
export type ToolRestFields = {
  /**
   * `GET`, `POST`, `PUT`, `PATCH` or `DELETE`.
   */
  method?: string;
  /**
   * Absolute URL; `{name}` placeholders only after the host and port.
   */
  url_template?: string;
  /**
   * Arguments sent as query parameters.
   */
  query_params?: Array<string>;
  /**
   * `none` or `json`.
   */
  body?: string;
  /**
   * Static, non-secret headers.
   */
  headers?: Array<StaticHeader>;
  /**
   * How the call authenticates.
   */
  auth?: HttpAuth;
  /**
   * Seconds before the call is abandoned.
   */
  timeout_seconds?: number;
  /**
   * Cap on the response text, in bytes.
   */
  max_response_bytes?: number;
};
