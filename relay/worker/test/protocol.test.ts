import { describe, expect, it } from "vitest";
import {
  decodeFrame,
  encodeFrame,
  encodeJson,
  encodeU32,
  F,
  frameJson,
  frameU32,
  HEADER_LEN,
  negotiateVersion,
  ProtocolError,
  subprotocolFor,
} from "../src/protocol";

describe("frame encoding", () => {
  it("lays out [type][u32 BE stream][payload]", () => {
    const f = encodeFrame(F.DATA, 0x01020304, new Uint8Array([9, 8, 7]));
    expect([...f]).toEqual([0x11, 1, 2, 3, 4, 9, 8, 7]);
    const d = decodeFrame(f);
    expect(d.type).toBe(F.DATA);
    expect(d.stream).toBe(0x01020304);
    expect([...d.payload]).toEqual([9, 8, 7]);
  });

  it("round-trips empty, JSON and u32 payloads", () => {
    const end = decodeFrame(encodeFrame(F.END, 7));
    expect(end.payload.byteLength).toBe(0);
    expect(encodeFrame(F.END, 7).byteLength).toBe(HEADER_LEN);

    const j = decodeFrame(encodeJson(F.RESPONSE, 3, { status: 201, headers: [["x-a", "ü"]] }));
    expect(frameJson<{ status: number; headers: [string, string][] }>(j)).toEqual({ status: 201, headers: [["x-a", "ü"]] });

    const w = decodeFrame(encodeU32(F.WINDOW, 5, 0xfffffffe));
    expect(frameU32(w)).toBe(0xfffffffe);
  });

  it("decodes from ArrayBuffer and offset views", () => {
    const bytes = encodeFrame(F.WS_TEXT, 42, new TextEncoder().encode("hi"));
    const padded = new Uint8Array(bytes.byteLength + 3);
    padded.set(bytes, 3);
    const view = padded.subarray(3);
    expect(decodeFrame(view).stream).toBe(42);
    expect(decodeFrame(bytes.slice().buffer).type).toBe(F.WS_TEXT);
  });

  it("supports the full u32 stream id range", () => {
    expect(decodeFrame(encodeFrame(F.OPEN, 0xffffffff)).stream).toBe(0xffffffff);
    expect(() => encodeFrame(F.OPEN, 0x1_0000_0000)).toThrow(ProtocolError);
    expect(() => encodeFrame(F.OPEN, -1)).toThrow(ProtocolError);
    expect(() => encodeFrame(256, 1)).toThrow(ProtocolError);
  });

  it("rejects malformed frames and payloads", () => {
    expect(() => decodeFrame(new Uint8Array([1, 0, 0]))).toThrow(ProtocolError);
    expect(() => frameJson(decodeFrame(encodeFrame(F.HELLO, 0, new TextEncoder().encode("not json"))))).toThrow(ProtocolError);
    expect(() => frameJson(decodeFrame(encodeFrame(F.HELLO, 0, new TextEncoder().encode("[1,2]"))))).toThrow(ProtocolError);
    expect(() => frameJson(decodeFrame(encodeFrame(F.HELLO, 0, new Uint8Array([0xff, 0xfe]))))).toThrow(ProtocolError);
    expect(() => frameU32(decodeFrame(encodeFrame(F.WINDOW, 1, new Uint8Array([1, 2]))))).toThrow(ProtocolError);
  });

  it("keeps type codes stable (wire contract with crates/devall-relay)", () => {
    expect(F).toMatchObject({
      HELLO: 1,
      ERROR: 2,
      QUOTA: 3,
      PING: 4,
      PONG: 5,
      GOAWAY: 6,
      USAGE: 7,
      TUNNEL_CONFIG: 8,
      OPEN: 0x10,
      DATA: 0x11,
      END: 0x12,
      RESPONSE: 0x13,
      RESPONSE_DATA: 0x14,
      RESPONSE_END: 0x15,
      RESET: 0x16,
      WINDOW: 0x17,
      WS_OPEN: 0x20,
      WS_ACCEPT: 0x21,
      WS_REJECT: 0x22,
      WS_TEXT: 0x23,
      WS_BINARY: 0x24,
      WS_CLOSE: 0x25,
      INBOX_EVENT: 0x30,
      INBOX_ACK: 0x31,
      INBOX_STATUS: 0x32,
      INBOX_CONFIG: 0x33,
    });
  });
});

describe("version negotiation", () => {
  it("picks the highest common version", () => {
    expect(negotiateVersion("devall-relay.1")).toBe(1);
    expect(negotiateVersion("devall-relay.3, devall-relay.1")).toBe(1);
    expect(negotiateVersion("chat, devall-relay.1")).toBe(1);
  });
  it("returns null without a common version", () => {
    expect(negotiateVersion(null)).toBeNull();
    expect(negotiateVersion("")).toBeNull();
    expect(negotiateVersion("devall-relay.9")).toBeNull();
    expect(negotiateVersion("devall-relay.x")).toBeNull();
  });
  it("formats the subprotocol", () => {
    expect(subprotocolFor(1)).toBe("devall-relay.1");
  });
});
