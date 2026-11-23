/**
 * Typed IPC error mapping.
 *
 * The Rust shell returns `{ code, message }` where `code` is the daemon's
 * protocol `ErrorCode` name. This module maps codes to static UI text without
 * ever carrying user text. Unknown codes fall back to a safe generic message
 * (issue #72): the raw daemon message is never echoed, since it could carry
 * user text or paths.
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
      return "Okkhor engine is not running. Typing keeps working; settings are read-only until it starts.";
    case "engine_missing":
      return "The Okkhor engine is missing. Re-extract the portable folder and keep bin next to Okkhor.exe.";
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
    case "InvalidWord":
      return "One or more word fields are invalid.";
    case "DuplicateWord":
      return "That word already exists.";
    case "TooManyWords":
      return "The word list is full.";
    case "TextTooLong":
      return "That text exceeds the insert limit.";
    case "ExportTooLarge":
      return "The export is too large.";
    case "InvalidRequest":
      return "The request was malformed.";
    case "UnknownMethod":
      return "That action is not supported.";
    case "Timeout":
      return "The daemon took too long to respond.";
    case "Unsupported":
      return "Not available in this milestone.";
    case "internal":
      return "Something went wrong.";
    default:
      // Unknown daemon code: never echo the raw message (it could carry user
      // text or paths); use the safe generic message instead.
      return "Something went wrong.";
  }
}
