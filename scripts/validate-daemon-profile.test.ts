// validate-daemon-profile.sh refuses a provisioning profile that cannot sign
// the daemon. These run it against decoded fixture profiles (macOS tools:
// PlistBuddy, plutil).
import { describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

const SCRIPT = join(dirname(new URL(import.meta.url).pathname), "validate-daemon-profile.sh");
const macTest = process.platform === "darwin" ? test : test.skip;

const CERT = Buffer.from("not a real certificate, only bytes to hash");
const CERT_SHA1 = createHash("sha1").update(CERT).digest("hex").toUpperCase();

interface Fields {
  appId?: string;
  team?: string;
  groups?: string[];
  expires?: string;
  certs?: Buffer[];
}

function profile(f: Fields = {}): string {
  const team = f.team ?? "F4M5J7YNLS";
  const appId = f.appId ?? `${team}.app.nodespace.daemon`;
  const groups = f.groups ?? [`${team}.app.nodespace.daemon`];
  const certs = f.certs ?? [CERT];
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>TeamIdentifier</key><array><string>${team}</string></array>
<key>ExpirationDate</key><date>${f.expires ?? "2030-01-01T00:00:00Z"}</date>
<key>DeveloperCertificates</key><array>${certs.map((c) => `<data>${c.toString("base64")}</data>`).join("")}</array>
<key>Entitlements</key><dict>
<key>com.apple.application-identifier</key><string>${appId}</string>
<key>keychain-access-groups</key><array>${groups.map((g) => `<string>${g}</string>`).join("")}</array>
</dict></dict></plist>`;
}

function run(f: Fields, env: Record<string, string> = {}) {
  const dir = mkdtempSync(join(tmpdir(), "profile-"));
  try {
    const path = join(dir, "p.plist");
    writeFileSync(path, profile(f));
    const out = Bun.spawnSync(["bash", SCRIPT, path, "--decoded"], {
      env: { PATH: process.env.PATH ?? "", SIGNING_CERT_SHA1: CERT_SHA1, VALIDATE_NOW: "1800000000", ...env },
      stdout: "pipe",
      stderr: "pipe",
    });
    return { code: out.exitCode, text: out.stdout.toString() + out.stderr.toString() };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

describe("validate-daemon-profile.sh", () => {
  macTest("accepts a profile for the App ID, the group, the team, not expired, with the signing certificate", () => {
    const r = run({});
    expect(r.text).toContain("is valid for F4M5J7YNLS.app.nodespace.daemon");
    expect(r.code).toBe(0);
  });

  macTest("accepts a team wildcard and a second certificate", () => {
    const r = run({ appId: "F4M5J7YNLS.*", groups: ["F4M5J7YNLS.*"], certs: [Buffer.from("other"), CERT] });
    expect(r.code).toBe(0);
  });

  macTest("refuses another App ID", () => {
    const r = run({ appId: "F4M5J7YNLS.app.other" });
    expect(r.code).not.toBe(0);
    expect(r.text).toContain("application identifier");
  });

  macTest("refuses another team", () => {
    const r = run({ team: "ABCDE12345", appId: "F4M5J7YNLS.app.nodespace.daemon" });
    expect(r.code).not.toBe(0);
    expect(r.text).toContain("not for team");
  });

  macTest("refuses a profile that does not grant the keychain access group", () => {
    const r = run({ groups: ["F4M5J7YNLS.app.something.else"] });
    expect(r.code).not.toBe(0);
    expect(r.text).toContain("keychain access group");
  });

  macTest("warns, and still passes, when the profile expires within 30 days", () => {
    // Expires 2027-01-15; judged at 2027-01-01.
    const soon = run({ expires: "2027-01-15T00:00:00Z" }, { VALIDATE_NOW: "1798761600" });
    expect(soon.code).toBe(0);
    expect(soon.text).toContain("::warning::");
    const later = run({ expires: "2030-01-01T00:00:00Z" });
    expect(later.text).not.toContain("::warning::");
  });

  macTest("refuses an expired profile", () => {
    const r = run({ expires: "2026-01-01T00:00:00Z" });
    expect(r.code).not.toBe(0);
    expect(r.text).toContain("expired");
  });

  macTest("refuses a profile that does not allow the signing certificate", () => {
    const r = run({ certs: [Buffer.from("someone else's certificate")] });
    expect(r.code).not.toBe(0);
    expect(r.text).toContain("signing certificate");
  });

  macTest("prints nothing of the profile's contents", () => {
    for (const f of [{}, { appId: "F4M5J7YNLS.app.other" }, { certs: [Buffer.from("x")] }]) {
      const r = run(f);
      expect(r.text).not.toContain(CERT.toString("base64"));
      expect(r.text).not.toContain("<plist");
    }
  });

  test("the release workflow checks the profile before using it and removes it afterwards", () => {
    const workflow = readFileSync(join(dirname(SCRIPT), "..", ".github", "workflows", "release.yml"), "utf8");
    expect(workflow).toContain("scripts/validate-daemon-profile.sh");
    expect(workflow).toContain("umask 077");
    expect(workflow).toContain('rm -f "$RUNNER_TEMP/nodespaced.provisionprofile"');
    expect(workflow.indexOf("validate-daemon-profile.sh")).toBeLessThan(workflow.indexOf('echo "NODESPACED_PROVISION_PROFILE='));
  });
});
