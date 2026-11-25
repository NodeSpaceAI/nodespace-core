// Tests for the public-intake GitHub utilities: the de-duplicated tracking
// issue a failing scheduled check files, and the client's issue listing it
// relies on. The team's own work is tracked in NodeSpace, so the scripts that
// create, edit, assign and set the status of issues no longer exist; the
// last suite pins that.
import { describe, expect, mock, spyOn, test } from "bun:test";
import { readFileSync } from "node:fs";
import { NodeSpaceGitHubManager } from "./gh-utils.ts";
import { GitHubClient } from "./github-client.ts";

// Covers NodeSpaceGitHubManager.findOrCreateTrackingIssue -- the shared
// dedup helper scheduled monitoring workflows (verify-macos-installer.yml,
// homebrew-drift-check.yml) call on failure instead of hand-rolled
// bash/jq. Extracted specifically because the hand-rolled version needed
// two rounds of manual review to catch a broken `gh issue list --jq --arg`
// invocation and a search-index eventual-consistency race -- exactly the
// class of bug a small, unit-tested function should catch before merge.
describe("NodeSpaceGitHubManager.findOrCreateTrackingIssue", () => {
  function makeStubClient(issues: Array<{ number: number; title: string; state?: string }>) {
    const listIssues = mock(async (_options: { state?: string }) =>
      issues.map((issue) => ({
        number: issue.number,
        title: issue.title,
        state: issue.state ?? "open",
        assignees: [],
        labels: [],
        body: "",
      })),
    );
    const addPRComment = mock(async (issueNumber: number, _body: string) => ({
      id: 999,
      url: `https://example.test/issues/${issueNumber}#comment`,
    }));
    const createIssue = mock(async (_title: string, _body: string, _labels?: string[]) => ({
      number: 4242,
      url: `https://example.test/issues/4242`,
    }));

    const client = { listIssues, addPRComment, createIssue } as unknown as GitHubClient;

    return { client, listIssues, addPRComment, createIssue };
  }

  test("comments on an existing open issue with an exact title match instead of creating a new one", async () => {
    const { client, listIssues, addPRComment, createIssue } = makeStubClient([
      { number: 100, title: "Some unrelated issue" },
      { number: 101, title: "macOS .pkg installer fails live Gatekeeper assessment" },
    ]);
    const manager = new NodeSpaceGitHubManager(client);

    const result = await manager.findOrCreateTrackingIssue({
      title: "macOS .pkg installer fails live Gatekeeper assessment",
      body: "Run: https://example.test/run/1",
    });

    expect(listIssues).toHaveBeenCalledWith({ state: "open" });
    expect(addPRComment).toHaveBeenCalledWith(101, "Run: https://example.test/run/1");
    expect(createIssue).not.toHaveBeenCalled();
    expect(result).toEqual({
      number: 101,
      url: "https://example.test/issues/101#comment",
      action: "commented",
    });
  });

  test("creates a new issue when no open issue has this exact title", async () => {
    const { client, addPRComment, createIssue } = makeStubClient([
      { number: 100, title: "Some unrelated issue" },
    ]);
    const manager = new NodeSpaceGitHubManager(client);

    const result = await manager.findOrCreateTrackingIssue({
      title: "Homebrew tap drift check failed (cask)",
      body: "Run: https://example.test/run/2",
      labels: ["foundation"],
    });

    expect(addPRComment).not.toHaveBeenCalled();
    expect(createIssue).toHaveBeenCalledWith(
      "Homebrew tap drift check failed (cask)",
      "Run: https://example.test/run/2",
      ["foundation"],
    );
    expect(result).toEqual({ number: 4242, url: "https://example.test/issues/4242", action: "created" });
  });

  test("requires an exact title match -- a title that merely contains or starts with it does not count", async () => {
    const { client, addPRComment, createIssue } = makeStubClient([
      { number: 100, title: "Homebrew tap drift check failed (cask) -- follow-up" },
      { number: 101, title: "Pre: Homebrew tap drift check failed (cask)" },
    ]);
    const manager = new NodeSpaceGitHubManager(client);

    await manager.findOrCreateTrackingIssue({
      title: "Homebrew tap drift check failed (cask)",
      body: "Run: https://example.test/run/3",
    });

    expect(addPRComment).not.toHaveBeenCalled();
    expect(createIssue).toHaveBeenCalledTimes(1);
  });

  test("picks the matching issue out of several open issues, not just the first one listed", async () => {
    const { client, addPRComment, createIssue } = makeStubClient([
      { number: 100, title: "Unrelated #1" },
      { number: 101, title: "Unrelated #2" },
      { number: 102, title: "Homebrew tap drift check failed (nodespace-cli formula)" },
    ]);
    const manager = new NodeSpaceGitHubManager(client);

    const result = await manager.findOrCreateTrackingIssue({
      title: "Homebrew tap drift check failed (nodespace-cli formula)",
      body: "Run: https://example.test/run/4",
    });

    expect(result.number).toBe(102);
    expect(addPRComment).toHaveBeenCalledWith(102, "Run: https://example.test/run/4");
    expect(createIssue).not.toHaveBeenCalled();
  });

  // Defense in depth: findOrCreateTrackingIssue asks GitHubClient#listIssues
  // for state: "open" and additionally checks issue.state === "open" itself
  // before matching. This covers that second check independently of the
  // server-side filter -- a closed issue with a matching title must never
  // be mistaken for the tracking issue to comment on, even if something
  // upstream (a stubbed/mocked client in a future test, a future API
  // contract change) ever returns one despite the open-only request.
  test("never matches a closed issue even if one with the same title is present", async () => {
    const { client, addPRComment, createIssue } = makeStubClient([
      { number: 100, title: "macOS .pkg installer fails live Gatekeeper assessment", state: "closed" },
    ]);
    const manager = new NodeSpaceGitHubManager(client);

    const result = await manager.findOrCreateTrackingIssue({
      title: "macOS .pkg installer fails live Gatekeeper assessment",
      body: "Run: https://example.test/run/5",
    });

    expect(addPRComment).not.toHaveBeenCalled();
    expect(createIssue).toHaveBeenCalledTimes(1);
    expect(result.action).toBe("created");
  });

  // Matches the wrap-and-log-then-rethrow pattern every other
  // NodeSpaceGitHubManager method uses (createIssue, editIssue, addComment,
  // ...): a failure mid-way through -- e.g. an existing issue was found but
  // commenting on it then fails -- must not vanish into main()'s generic
  // top-level catch with no indication of which tracking issue it was
  // trying to update.
  test("logs which tracking issue it was updating and rethrows if the API call fails", async () => {
    const listIssues = mock(async () => [
      {
        number: 101,
        title: "macOS .pkg installer fails live Gatekeeper assessment",
        state: "open",
        assignees: [],
        labels: [],
        body: "",
      },
    ]);
    const addPRComment = mock(async () => {
      throw new Error("API rate limited");
    });
    const client = { listIssues, addPRComment } as unknown as GitHubClient;
    const manager = new NodeSpaceGitHubManager(client);
    const errorSpy = spyOn(console, "error").mockImplementation(() => {});

    try {
      await expect(
        manager.findOrCreateTrackingIssue({
          title: "macOS .pkg installer fails live Gatekeeper assessment",
          body: "Run: https://example.test/run/6",
        }),
      ).rejects.toThrow("API rate limited");

      expect(errorSpy).toHaveBeenCalledWith(
        expect.stringContaining("macOS .pkg installer fails live Gatekeeper assessment"),
      );
    } finally {
      errorSpy.mockRestore();
    }
  });
});

// Covers GitHubClient.listIssues directly (stubbed octokit): pagination and
// pull-request filtering, both load-bearing for
// NodeSpaceGitHubManager.findOrCreateTrackingIssue's exact-title dedup —
// an unpaginated call could silently miss an older still-open tracking
// issue past the first page, and an unfiltered one could match an open
// PR's title instead of a real issue's.
describe("GitHubClient.listIssues", () => {
  function makeClientWithStubbedOctokit(pages: Array<Array<Record<string, unknown>>>) {
    // `page` param, 1-indexed, matching REST convention.
    const listForRepo = mock(async (params: { page?: number }) => ({
      data: pages[(params.page ?? 1) - 1] ?? [],
    }));

    // A faithful-enough stand-in for octokit's real paginate: calls the
    // given route with the given params plus an incrementing `page`,
    // concatenating every page's `data`, stopping once a page comes back
    // shorter than `per_page` (or empty). Calling the real `listForRepo`
    // mock through this (rather than just returning a canned flattened
    // array) exercises the actual route reference and params listIssues
    // passes, the way real pagination would -- a wrong route or dropped
    // param would be caught here, not silently invisible to the test.
    const paginate = mock(
      async (
        route: (params: Record<string, unknown>) => Promise<{ data: unknown[] }>,
        params: Record<string, unknown>,
      ) => {
        const perPage = (params.per_page as number | undefined) ?? 100;
        const results: unknown[] = [];
        let page = 1;
        for (;;) {
          const { data } = await route({ ...params, page });
          results.push(...data);
          if (data.length < perPage) break;
          page += 1;
        }
        return results;
      },
    );

    const client = new GitHubClient("stub-token");
    (client as unknown as { octokit: unknown }).octokit = {
      paginate,
      rest: { issues: { listForRepo } },
    };

    return { client, paginate, listForRepo };
  }

  test("returns issues spanning more than one page", async () => {
    const page1 = Array.from({ length: 100 }, (_, i) => ({ number: i + 1, title: `Issue ${i + 1}`, state: "open" }));
    const page2 = [{ number: 101, title: "Issue 101 (past the first page)", state: "open" }];
    const { client, paginate, listForRepo } = makeClientWithStubbedOctokit([page1, page2]);

    const issues = await client.listIssues({ state: "open" });

    expect(paginate).toHaveBeenCalledTimes(1);
    // paginate must be given the real listForRepo route (not some other
    // function) and the caller's state filter, or dedup could silently
    // end up listing the wrong thing entirely.
    expect(paginate.mock.calls[0][0]).toBe(listForRepo);
    expect(paginate.mock.calls[0][1]).toMatchObject({ state: "open" });
    expect(listForRepo).toHaveBeenCalledTimes(2);
    expect(issues).toHaveLength(101);
    expect(issues.map((i) => i.number)).toContain(101);
  });

  test("filters out pull requests, which the underlying endpoint returns mixed in with issues", async () => {
    const { client } = makeClientWithStubbedOctokit([
      [
        { number: 1, title: "A real issue", state: "open" },
        { number: 2, title: "An open PR with a coincidentally matching title", state: "open", pull_request: {} },
      ],
    ]);

    const issues = await client.listIssues({ state: "open" });

    expect(issues).toHaveLength(1);
    expect(issues[0].number).toBe(1);
  });
});

// GitHub issues are the public intake only: the package scripts that file,
// edit, assign, unassign, list or set the board status of an issue are gone,
// and the scripts that remain say so.
describe("gh:* scripts are intake-only", () => {
  const pkg = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")) as {
    scripts: Record<string, string>;
  };
  const ghScripts = Object.keys(pkg.scripts)
    .filter((name) => name.startsWith("gh:"))
    .sort();

  test("only view, comment, pr, tracking-issue and help remain", () => {
    expect(ghScripts).toEqual(["gh:comment", "gh:help", "gh:pr", "gh:tracking-issue", "gh:view"]);
  });

  test("the manager has no method that creates, edits, assigns or sets the status of an issue", () => {
    const methods = Object.getOwnPropertyNames(NodeSpaceGitHubManager.prototype);
    for (const removed of ["createIssue", "editIssue", "assignIssues", "unassignIssues", "updateIssueStatus", "listIssues"]) {
      expect(methods).not.toContain(removed);
    }
  });

  test("the help text states that GitHub issues are the public intake", () => {
    const result = Bun.spawnSync(["bun", "run", "scripts/gh-utils.ts", "help"], {
      cwd: new URL("..", import.meta.url).pathname,
    });
    const out = result.stdout.toString();
    expect(out).toContain("public intake");
    expect(out).not.toContain("gh:assign");
    expect(out).not.toContain("gh:status");
  });
});
