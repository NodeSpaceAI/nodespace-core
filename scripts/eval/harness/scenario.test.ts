import { afterAll, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { isUsersOwn } from "./scenario.ts";

describe("isUsersOwn", () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "harness-scenario-")));
  const home = join(root, "home");

  mkdirSync(join(home, ".nodespace", "database"), { recursive: true });
  mkdirSync(join(root, "isolated", ".nodespace"), { recursive: true });
  symlinkSync(join(home, ".nodespace"), join(root, "link-to-own"));

  test("the user's folder and anything in it is the user's own", () => {
    expect(isUsersOwn(join(home, ".nodespace"), home)).toBe(true);
    expect(isUsersOwn(join(home, ".nodespace", "database", "nodespace.db"), home)).toBe(true);
    expect(isUsersOwn(join(home, ".nodespace", "daemon.sock"), home)).toBe(true);
  });

  test("another spelling of the same folder is still the user's own", () => {
    expect(isUsersOwn(`${home}/.nodespace/`, `${home}/`)).toBe(true);
    expect(isUsersOwn(join(root, "link-to-own"), home)).toBe(true);
    expect(isUsersOwn(join(root, "link-to-own", "database", "nodespace.db"), home)).toBe(true);
  });

  test("an isolated home is not", () => {
    expect(isUsersOwn(join(root, "isolated", ".nodespace"), home)).toBe(false);
    expect(isUsersOwn(join(root, "isolated", ".nodespace", "daemon.sock"), home)).toBe(false);
    // A sibling whose name only begins the same way.
    expect(isUsersOwn(join(home, ".nodespace-other", "daemon.sock"), home)).toBe(false);
  });

  test("with no home to compare against nothing is called the user's own", () => {
    expect(isUsersOwn(join(home, ".nodespace"), "")).toBe(false);
  });

  afterAll(() => rmSync(root, { recursive: true, force: true }));
});
