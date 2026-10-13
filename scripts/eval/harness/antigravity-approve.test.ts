import { describe, expect, it } from "bun:test";
import { decide } from "./antigravity-approve.ts";

const run = (CommandLine: unknown) => decide({ toolCall: { name: "run_command", args: { CommandLine } } }).decision;

describe("the Antigravity scenario approver", () => {
  it("approves the commands the scenario uses", () => {
    for (const line of [
      "nodespace skill guidance",
      "nodespace node context 3e9a7c14-5d28 --json 2>&1",
      "nodespace query run 'Ready tasks' --filters '[{\"type\":\"relationship\"}]'",
      "node test.js",
      "git add slug.js test.js && git commit -m \"widget: collapse repeated dashes\"",
      // A multi-line message inside quotes is one word, not three commands.
      'git commit -m "widget: collapse repeated dashes\n\nChecked-by: osprey"',
      'echo "a;b"',
      "ls -la && cat slug.js | head -20",
      "sh soak.sh",
      "grep -n slugify slug.js > /dev/null",
    ]) {
      expect(run(line), line).toBe("auto_approve");
    }
  });

  it("asks about a second command hidden behind a quote or an operator", () => {
    for (const line of [
      "echo a & id",
      `echo "a'b" && id && echo "c'd"`,
      "echo \\' ; id ; echo \\'",
      "echo hi <(id)",
      "echo $(id)",
      "echo `id`",
      'echo "$(id)"',
      "echo hi; id",
      "echo hi\nid",
      "echo hi || id",
      "echo hi | id",
      "(id)",
    ]) {
      expect(run(line), line).toBe("ask");
    }
  });

  it("asks about a write outside the checkout", () => {
    for (const line of ["echo hi > ~/.zshrc", "cat f >> /etc/hosts", "cat f 2> /tmp/x", "sort -o /etc/x f", "sed -i s/a/b/ ~/.zshrc"]) {
      expect(run(line), line).toBe("ask");
    }
  });

  it("asks about a program the line names a path to, or an assignment before it", () => {
    for (const line of ["/tmp/evil/git status", "./node test.js", "PATH=/tmp/evil git status", "LD_PRELOAD=/tmp/x.so ls"]) {
      expect(run(line), line).toBe("ask");
    }
  });

  it("asks about git run with a program or a file to write", () => {
    for (const line of [
      'git -c core.pager="sh -c id" log',
      'git config alias.x "!id"',
      "git log --output=/etc/x",
      "git diff --exec-path=/tmp/x",
      "git push origin main",
    ]) {
      expect(run(line), line).toBe("ask");
    }
  });

  it("asks about code written on the line", () => {
    for (const line of ["node -e 'process.exit(0)'", "node -p 1", "sh -c id", "sh"]) {
      expect(run(line), line).toBe("ask");
    }
  });

  it("fails closed on input it cannot read", () => {
    expect(decide(null).decision).toBe("ask");
    expect(decide({}).decision).toBe("ask");
    expect(decide({ toolCall: {} }).decision).toBe("ask");
    expect(decide({ toolCall: { name: "run_command" } }).decision).toBe("ask");
    expect(run("")).toBe("ask");
    expect(run(42)).toBe("ask");
    expect(run("echo 'unterminated")).toBe("ask");
  });

  it("leaves a tool that is not a shell command to the session's mode", () => {
    expect(decide({ toolCall: { name: "write_to_file", args: {} } }).decision).toBe("allow");
  });
});
