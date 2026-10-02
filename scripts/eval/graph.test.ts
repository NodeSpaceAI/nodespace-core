/**
 * Unit tests for the snapshot's type selection.
 *
 * The harness drives every scenario through a chat node of its own, which
 * gains message nodes during the scored turn. If a snapshot queried the
 * chat's type or its messages', every read-only scenario would report a
 * changed or new node and fail `expectNoWrites` on the harness's own
 * bookkeeping.
 */

import { describe, expect, test } from "bun:test";
import { CHAT_NODE_TYPE, MESSAGE_NODE_TYPE } from "../aichat.ts";
import { snapshotQueryTypes } from "./graph.ts";

describe("snapshotQueryTypes", () => {
  test("leaves out the whole chat family and date containers", () => {
    const schemas = [
      "ai-chat",
      "ai-chat-native",
      "ai-chat-pty",
      "ai-chat-message",
      "date",
      "task",
      "text",
    ];
    expect(snapshotQueryTypes([], schemas)).toEqual(["task", "text"]);
  });

  test("a predicted type is queried once, and scaffolding is dropped from it too", () => {
    expect(
      snapshotQueryTypes(["invoice", "task", "ai-chat-native"], ["task"]),
    ).toEqual(["invoice", "task"]);
  });

  test("the type the harness creates its chat as is excluded", () => {
    // `aichat.ts` creates the chat every scenario runs through; a change to
    // its type must not leave the snapshot counting it.
    expect(snapshotQueryTypes([], [CHAT_NODE_TYPE])).toEqual([]);
  });

  test("the type the harness creates its messages as is excluded", () => {
    expect(snapshotQueryTypes([MESSAGE_NODE_TYPE], [MESSAGE_NODE_TYPE])).toEqual([]);
  });
});
