import { describe, expect, test } from "vitest";
import {
  DAEMON_UNAVAILABLE,
  describeError,
  getErrorCode,
  getErrorMessage,
  isDaemonUnavailable,
} from "../lib/errors";

describe("typed IPC error mapping", () => {
  test("daemon_unavailable is detected from object and string forms", () => {
    expect(isDaemonUnavailable({ code: DAEMON_UNAVAILABLE, message: "x" })).toBe(true);
    expect(isDaemonUnavailable(DAEMON_UNAVAILABLE)).toBe(true);
    expect(isDaemonUnavailable({ code: "InvalidLayout", message: "x" })).toBe(false);
    expect(isDaemonUnavailable(null)).toBe(false);
    expect(isDaemonUnavailable(undefined)).toBe(false);
  });

  test("getErrorCode falls back to internal for unknown shapes", () => {
    expect(getErrorCode({ code: "Timeout", message: "m" })).toBe("Timeout");
    expect(getErrorCode("oops")).toBe("oops");
    expect(getErrorCode(null)).toBe("internal");
    expect(getErrorCode({})).toBe("internal");
    expect(getErrorCode({ code: 42 })).toBe("internal");
  });

  test("getErrorMessage never returns empty for error objects", () => {
    expect(getErrorMessage({ code: "X", message: "hello" })).toBe("hello");
    expect(getErrorMessage("plain")).toBe("plain");
    expect(getErrorMessage(null)).toBe("Something went wrong.");
    expect(getErrorMessage({})).toBe("Something went wrong.");
  });

  test("describeError uses static text per code", () => {
    expect(describeError({ code: "daemon_unavailable", message: "" })).toContain(
      "engine is not running",
    );
    expect(describeError({ code: "engine_missing", message: "" })).toContain(
      "engine is missing",
    );
    expect(describeError({ code: "ProtocolMismatch", message: "" })).toContain("Restart Okkhor");
    expect(describeError({ code: "InvalidLayout", message: "" })).toContain("Unknown keyboard layout");
    expect(describeError({ code: "InvalidHotkey", message: "" })).toContain("not valid");
    expect(describeError({ code: "Timeout", message: "" })).toContain("took too long");
    expect(describeError({ code: "Unsupported", message: "" })).toContain("milestone");
    expect(describeError({ code: "DuplicateWord", message: "" })).toContain("already exists");
    expect(describeError({ code: "InvalidWord", message: "" })).toContain("word fields");
    expect(describeError({ code: "TooManyWords", message: "" })).toContain("full");
    expect(describeError({ code: "TextTooLong", message: "" })).toContain("insert limit");
  });

  test("unknown codes fall back to a safe generic message, never raw text", () => {
    // Issue #72: the raw daemon message could carry user text or paths.
    expect(
      describeError({ code: "Weird", message: "weird detail with user words আমার" }),
    ).toBe("Something went wrong.");
    expect(describeError("mystery string")).toBe("Something went wrong.");
    expect(describeError(null)).toBe("Something went wrong.");
  });
});
