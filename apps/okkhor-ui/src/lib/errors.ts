/**
 * Typed IPC error mapping.
 *
 * The Rust shell returns `{ code, message }` where `code` is one of:
 * `daemon_unavailable` (pipe/token/connection failure), the daemon's
 * protocol `ErrorCode` name verbatim (e.g. `InvalidLayout`), or
 * `internal`. This module maps those codes to UI behavior without
 * ever carrying user text.
 */

export const DAEMON_UNAVAILABLE = "daemon_unavailable" as const;

export interface IpcError {
  code: string;
  message: string;
}

export function isDaemonUnavailable(error: unknown): boolean {
  return getErrorCode(error) === DAEMON_UNAVAILABLE;
}

export function getErrorCode(error: unknown): string {
  if (typeof error === "string") {
    return error;
  }
  if (typeof error === "object" && error !== null && "code" in error) {
    const code = (error as { code?: unknown }).code;
    if (typeof code === "string" && code.length > 0) {
      return code;
    }
  }
  return "internal";
}

export function getErrorMessage(error: unknown): string {
  if (typeof error === "string") {
    return error;
  }
  if (typeof error === "object" && error !== null && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string" && message.length > 0) {
      return message;
    }
  }
  return "Something went wrong.";
}

/** Human-readable one-liner for a typed error code (static text only). */
export function describeError(error: unknown): string {
  switch (getErrorCode(error)) {
    case DAEMON_UNAVAILABLE:
      return "Okkhor daemon is not running. Typing keeps working; settings are read-only until you retry.";
    case "ProtocolMismatch":
      return "UI and daemon versions disagree. Restart Okkhor.";
    case "Unauthorized":
      return "The UI session is not authorized. Restart Okkhor.";
    case "InvalidLayout":
      return "Unknown keyboard layout.";
    case "InvalidHotkey":
      return "That hotkey is not valid.";
    case "InvalidSettings":
      return "One or more settings values are invalid.";
    case "Timeout":
      return "The daemon took too long to respond.";
    case "Unsupported":
      return "Not available in this milestone.";
    default:
      return getErrorMessage(error);
  }
}
