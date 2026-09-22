// Regression test: `bun run gh:assign <n> "@me"` and
// `bun run gh:unassign <n> "@me"` used to resolve "@me" to the hardcoded
// literal string "malibio" regardless of who was actually authenticated via
// `gh`, then print a success message even though the wrong account got
// assigned. This silently mis-assigned real issues to the wrong GitHub
// account.
//
// This exercises NodeSpaceGitHubManager.assignIssues/unassignIssues against a
// stubbed GitHubClient (constructor-injected) so the assertions don't depend
// on which account actually runs this suite -- the stub's resolved login is
// deliberately something no real account will ever be, so a regression back
// to a hardcoded literal fails loudly no matter who's authenticated.
import { describe, expect, mock, test } from "bun:test";
import { NodeSpaceGitHubManager } from "./gh-utils.ts";
import { GitHubClient } from "./github-client.ts";

const RESOLVED_LOGIN = "totally-unrelated-test-account";

function makeStubClient() {
  const getAuthenticatedUser = mock(async () => RESOLVED_LOGIN);
  const assignIssues = mock(async (issueNumbers: number[], _assignees: string[]) =>
    issueNumbers.map((issueNumber) => ({ issueNumber, success: true })),
  );
  const unassignIssues = mock(async (issueNumbers: number[], _assignees: string[]) =>
    issueNumbers.map((issueNumber) => ({ issueNumber, success: true })),
  );

  const client = {
    getAuthenticatedUser,
    assignIssues,
    unassignIssues,
  } as unknown as GitHubClient;

  return { client, getAuthenticatedUser, assignIssues, unassignIssues };
}

describe("NodeSpaceGitHubManager.assignIssues", () => {
  test('"@me" resolves via the authenticated-user API, not a hardcoded literal', async () => {
    const { client, getAuthenticatedUser, assignIssues } = makeStubClient();
    const manager = new NodeSpaceGitHubManager(client);

    await manager.assignIssues([2288], "@me");

    expect(getAuthenticatedUser).toHaveBeenCalledTimes(1);
    expect(assignIssues).toHaveBeenCalledWith([2288], [RESOLVED_LOGIN]);

    // The exact regression this guards: the buggy code passed ["malibio"]
    // unconditionally for "@me", no matter who was authenticated.
    const [, assignedTo] = assignIssues.mock.calls[0] as [number[], string[]];
    expect(assignedTo).not.toContain("malibio");
    expect(assignedTo).toEqual([RESOLVED_LOGIN]);
  });

  test("an explicit username bypasses @me resolution entirely", async () => {
    const { client, getAuthenticatedUser, assignIssues } = makeStubClient();
    const manager = new NodeSpaceGitHubManager(client);

    await manager.assignIssues([2288], "@someone-else");

    expect(getAuthenticatedUser).not.toHaveBeenCalled();
    expect(assignIssues).toHaveBeenCalledWith([2288], ["someone-else"]);
  });
});

describe("NodeSpaceGitHubManager.unassignIssues", () => {
  test('"@me" resolves via the authenticated-user API, not a hardcoded literal', async () => {
    const { client, getAuthenticatedUser, unassignIssues } = makeStubClient();
    const manager = new NodeSpaceGitHubManager(client);

    await manager.unassignIssues([2288], "@me");

    expect(getAuthenticatedUser).toHaveBeenCalledTimes(1);
    expect(unassignIssues).toHaveBeenCalledWith([2288], [RESOLVED_LOGIN]);

    const [, assignedFrom] = unassignIssues.mock.calls[0] as [number[], string[]];
    expect(assignedFrom).not.toContain("malibio");
    expect(assignedFrom).toEqual([RESOLVED_LOGIN]);
  });

  test("an explicit username bypasses @me resolution entirely", async () => {
    const { client, getAuthenticatedUser, unassignIssues } = makeStubClient();
    const manager = new NodeSpaceGitHubManager(client);

    await manager.unassignIssues([2288], "@someone-else");

    expect(getAuthenticatedUser).not.toHaveBeenCalled();
    expect(unassignIssues).toHaveBeenCalledWith([2288], ["someone-else"]);
  });
});

// Regression test: an issue that was never added to the ProjectV2 board had no
// project item, so `getItemIdForIssue` returned null and every
// `bun run gh:status <n> "..."` call on it failed with "Issue not found in
// project" — including the ones CLAUDE.md's startup sequence makes mandatory.
// Nothing in the tooling added issues to the board, so this hit every newly
// filed issue (#2376, #2384, #2389, #2390, #2396 were all missing).
//
// Two halves are covered here: creation now places the issue on the board, and
// a status update self-heals a missing row instead of refusing. The second
// matters most — it back-fills issues created before the first half existed.
describe("GitHubClient project-board membership", () => {
  const ISSUE_NUMBER = 2390;
  const ISSUE_NODE_ID = "I_kwDOtestnode";
  const NEW_ITEM_ID = "PVTI_lADOnewitem";

  function makeClientWithStubbedOctokit(options: { alreadyOnBoard: boolean }) {
    const graphqlCalls: Array<{ query: string; vars: Record<string, unknown> }> = [];

    const graphql = mock(async (query: string, vars: Record<string, unknown>) => {
      graphqlCalls.push({ query, vars });
      if (query.includes("addProjectV2ItemById")) {
        return { addProjectV2ItemById: { item: { id: NEW_ITEM_ID } } };
      }
      if (query.includes("updateProjectV2ItemFieldValue")) {
        return { updateProjectV2ItemFieldValue: { projectV2Item: { id: NEW_ITEM_ID } } };
      }
      // getProjectItems() paging query.
      return {
        organization: {
          projectV2: {
            items: {
              nodes: options.alreadyOnBoard
                ? [{ id: "PVTI_existing", content: { number: ISSUE_NUMBER } }]
                : [],
              pageInfo: { hasNextPage: false, endCursor: null },
            },
          },
        },
      };
    });

    const issuesCreate = mock(async () => ({
      data: { number: ISSUE_NUMBER, html_url: "https://example.test/i", node_id: ISSUE_NODE_ID },
    }));
    const issuesGet = mock(async () => ({ data: { node_id: ISSUE_NODE_ID } }));

    const client = new GitHubClient("stub-token");
    (client as unknown as { octokit: unknown }).octokit = {
      graphql,
      rest: { issues: { create: issuesCreate, get: issuesGet } },
    };

    return { client, graphql, graphqlCalls, issuesCreate, issuesGet };
  }

  test("creating an issue adds it to the project board", async () => {
    const { client, graphqlCalls } = makeClientWithStubbedOctokit({ alreadyOnBoard: false });

    const issue = await client.createIssue("Title", "Body");

    expect(issue.number).toBe(ISSUE_NUMBER);
    expect(issue.addedToProject).toBe(true);

    const add = graphqlCalls.find((c) => c.query.includes("addProjectV2ItemById"));
    expect(add).toBeDefined();
    expect(add!.vars.contentId).toBe(ISSUE_NODE_ID);
  });

  test("a failed board add still returns the created issue", async () => {
    const { client } = makeClientWithStubbedOctokit({ alreadyOnBoard: false });
    (client as unknown as { octokit: { graphql: unknown } }).octokit.graphql = mock(async () => {
      throw new Error("board unreachable");
    });

    const issue = await client.createIssue("Title", "Body");

    // The issue exists on GitHub regardless — reporting failure by throwing
    // would lose the number the caller actually needs.
    expect(issue.number).toBe(ISSUE_NUMBER);
    expect(issue.addedToProject).toBe(false);
  });

  test("a status update adds a missing issue to the board instead of failing", async () => {
    const { client, graphqlCalls } = makeClientWithStubbedOctokit({ alreadyOnBoard: false });

    const results = await client.updateIssueStatus([ISSUE_NUMBER], "Done");

    expect(results).toEqual([{ issueNumber: ISSUE_NUMBER, success: true }]);

    // The exact regression: this used to short-circuit to
    // { success: false, error: "Issue not found in project" }.
    expect(results[0].error).toBeUndefined();

    const add = graphqlCalls.find((c) => c.query.includes("addProjectV2ItemById"));
    expect(add).toBeDefined();

    // The status write must target the item the add returned.
    const update = graphqlCalls.find((c) => c.query.includes("updateProjectV2ItemFieldValue"));
    expect(update!.vars.itemId).toBe(NEW_ITEM_ID);
  });

  test("an issue already on the board is not re-added", async () => {
    const { client, graphqlCalls } = makeClientWithStubbedOctokit({ alreadyOnBoard: true });

    const results = await client.updateIssueStatus([ISSUE_NUMBER], "In Progress");

    expect(results).toEqual([{ issueNumber: ISSUE_NUMBER, success: true }]);
    expect(graphqlCalls.find((c) => c.query.includes("addProjectV2ItemById"))).toBeUndefined();

    const update = graphqlCalls.find((c) => c.query.includes("updateProjectV2ItemFieldValue"));
    expect(update!.vars.itemId).toBe("PVTI_existing");
  });
});

// Covers NodeSpaceGitHubManager.findOrCreateTrackingIssue -- the shared
// dedup helper scheduled monitoring workflows (verify-macos-installer.yml,
// homebrew-drift-check.yml) call on failure instead of hand-rolled
// bash/jq. Extracted specifically because the hand-rolled version needed
// two rounds of manual review to catch a broken `gh issue list --jq --arg`
// invocation and a search-index eventual-consistency race -- exactly the
// class of bug a small, unit-tested function should catch before merge.
describe("NodeSpaceGitHubManager.findOrCreateTrackingIssue", () => {
  function makeStubClient(openIssues: Array<{ number: number; title: string }>) {
    const listIssues = mock(async (_options: { state?: string }) =>
      openIssues.map((issue) => ({
        number: issue.number,
        title: issue.title,
        state: "open",
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
      addedToProject: true,
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
});
