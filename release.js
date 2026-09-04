#!/usr/bin/env node

const { execSync } = require("child_process");
const fs = require("fs");
const os = require("os");
const path = require("path");
const readline = require("node:readline/promises");

// ---------------------------------------------------------------------------
// Arguments
//
//   node release.js                 # patch bump  (1.0.11 -> 1.0.12)
//   node release.js --minor         # minor bump  (1.0.11 -> 1.1.0)
//   node release.js --major         # major bump  (1.0.11 -> 2.0.0)
//   node release.js 2.0.0-rc.1      # explicit version (overrides bump flag)
//   node release.js --minor --dry   # validate without publishing
//   node release.js --yes           # skip the confirmation prompt
//
// Pre-releases go through the explicit-version form. crates.io treats them as
// opt-in (a `2.0` requirement never resolves to `2.0.0-rc.1`), the GitHub
// release is flagged as a prerelease, and a later `node release.js` finalizes
// it to the base version (2.0.0-rc.1 -> 2.0.0) rather than skipping past it.
// ---------------------------------------------------------------------------
const args = process.argv.slice(2);
const dryRun = args.includes("--dry");
const skipConfirm = args.includes("--yes") || args.includes("-y");
const bumpKind = args.includes("--major")
  ? "major"
  : args.includes("--minor")
  ? "minor"
  : "patch";
// First non-flag argument is treated as an explicit version override.
const explicitVersion = args.find((arg) => !arg.startsWith("-"));

if (explicitVersion && !/^\d+\.\d+\.\d+(-.*)?$/.test(explicitVersion)) {
  console.error(
    "Invalid version format. Use semantic versioning (e.g., 1.2.3 or 1.2.3-beta.1)"
  );
  process.exit(1);
}

// Cached GitHub owner/repo parsed from the origin remote.
let repoInfo = null;

// cargo keeps one package-cache lock for the whole user, not one per workspace.
const packageCacheLock = path.join(
  process.env.CARGO_HOME || path.join(os.homedir(), ".cargo"),
  ".package-cache"
);

// cargo-workspaces reads the registry through the `crates-index` crate, which
// fails fast on a held lock instead of blocking the way cargo itself does. A
// `cargo check` in *any other* workspace is therefore enough to abort a publish
// with `crates index error: failed to obtain lock file '.../.package-cache'`.
const PACKAGE_CACHE_LOCK_ERROR = /failed to obtain lock file/i;
const CARGO_LOCK_ATTEMPTS = 3;
const CARGO_LOCK_WAIT_MS = 15 * 60 * 1000;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// Run a shell command. `dryRun: true` makes it a no-op (logged) during --dry.
// `allowFailure: true` returns null instead of exiting on a non-zero exit.
function run(command, options = {}) {
  try {
    if (options.dryRun && dryRun) {
      console.log(`   [DRY RUN] Would execute: ${command}`);
      return "";
    }
    return execSync(command, { encoding: "utf8", ...options }).trim();
  } catch (error) {
    if (!options.allowFailure) {
      console.error(`Command failed: ${command}`);
      console.error(error.message);
      process.exit(1);
    }
    return null;
  }
}

// PIDs currently holding the cargo package-cache lock open, excluding our own.
// `lsof` exits non-zero when nothing has the file open; if it is missing
// entirely we also get null, and treating that as "free" is the right
// degradation — runCargoLive still retries on the actual lock error.
function packageCacheHolders() {
  if (!fs.existsSync(packageCacheLock)) return [];
  const pids = run(`lsof -t ${packageCacheLock}`, { allowFailure: true });
  if (!pids) return [];
  return pids.split("\n").filter(Boolean);
}

// Block until no other cargo process holds the package-cache lock. Bounded:
// waiting forever on someone's hung build is worse than a clear error, and the
// caller retries anyway.
function waitForPackageCache(maxWaitMs = CARGO_LOCK_WAIT_MS) {
  let holders = packageCacheHolders();
  if (!holders.length) return;

  console.log(`\n⏳ Waiting for the cargo package-cache lock (${packageCacheLock})`);
  console.log(`   Held by pid(s): ${holders.join(", ")}`);
  console.log("   A cargo build in any other workspace will block this release.");

  const deadline = Date.now() + maxWaitMs;
  while (holders.length && Date.now() < deadline) {
    run("sleep 5");
    holders = packageCacheHolders();
  }

  if (holders.length) {
    console.warn(
      `⚠️  Lock still held after ${Math.round(maxWaitMs / 60000)} min — trying anyway.`
    );
    return;
  }
  console.log("✅ Package-cache lock is free");
}

// Stream a cargo command's output straight to the terminal (for long-running,
// chatty commands like `cargo ws publish` where live progress matters), and
// retry it when it dies on the package-cache lock.
//
// Retrying is safe because every cargo step here is idempotent: `cargo ws
// publish` skips crates already on crates.io, and the version bump is guarded
// by the `alreadyBumped` check in release().
function runCargoLive(command, options = {}) {
  if (options.dryRun && dryRun) {
    console.log(`   [DRY RUN] Would execute: ${command}`);
    return "";
  }

  const logFile = `/tmp/release-cargo-${process.pid}.log`;
  const cleanup = () => fs.existsSync(logFile) && fs.unlinkSync(logFile);

  for (let attempt = 1; attempt <= CARGO_LOCK_ATTEMPTS; attempt++) {
    waitForPackageCache();
    try {
      // `tee` keeps the live output while still capturing it, so a lock failure
      // can be told apart from a real publish failure. `pipefail` makes the
      // pipeline report cargo's exit status rather than tee's. The command is
      // wrapped in a group so the redirect covers all of it — `a; b 2>&1 | tee`
      // would capture only `b`. Newlines, not `;`, so a command that already
      // ends in a semicolon stays valid.
      execSync(`set -o pipefail\n{ ${command}\n} 2>&1 | tee ${logFile}`, {
        stdio: "inherit",
        shell: "/bin/bash",
        ...options,
      });
      const output = fs.existsSync(logFile)
        ? fs.readFileSync(logFile, "utf8")
        : "";
      cleanup();
      return output;
    } catch (error) {
      const output = fs.existsSync(logFile)
        ? fs.readFileSync(logFile, "utf8")
        : "";
      cleanup();
      if (!PACKAGE_CACHE_LOCK_ERROR.test(output) || attempt === CARGO_LOCK_ATTEMPTS) {
        throw error;
      }
      console.warn(
        `\n⚠️  Lost the cargo package-cache lock ` +
          `(attempt ${attempt}/${CARGO_LOCK_ATTEMPTS}). Retrying...`
      );
    }
  }
}

// Ask a yes/no question on stdin. Only an explicit "y" is a yes, so a
// non-interactive stdin (EOF) declines rather than proceeding.
async function ask(question) {
  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
  });
  const answer = await rl.question(question);
  rl.close();
  return answer.trim().toLowerCase() === "y";
}

function getRepoInfo() {
  if (repoInfo) return repoInfo;
  const remoteUrl = run("git remote get-url origin", { allowFailure: true });
  const match = remoteUrl && remoteUrl.match(/github\.com[:/]([^/]+)\/([^.]+)/);
  repoInfo = match ? { owner: match[1], repo: match[2] } : null;
  return repoInfo;
}

// Read the current workspace version. All crates inherit it via
// `version.workspace = true`, so the canonical source is the root
// Cargo.toml's [workspace.package] section.
function currentVersion() {
  const m = fs
    .readFileSync("Cargo.toml", "utf8")
    .match(/^\[workspace\.package\][\s\S]*?^version = "(.*)"$/m);
  if (!m) {
    console.error(
      "❌ Could not read current version from [workspace.package] in Cargo.toml"
    );
    process.exit(1);
  }
  return m[1];
}

// Compute the next version from a bump keyword. A patch bump off a pre-release
// finalizes it (2.0.0-rc.1 -> 2.0.0) instead of stepping past the version the
// release candidate was a candidate *for*.
function nextVersion(cur, kind) {
  const base = cur.split("-")[0];
  const [maj, min, pat] = base.split(".").map(Number);
  if (kind === "major") return `${maj + 1}.0.0`;
  if (kind === "minor") return `${maj}.${min + 1}.0`;
  return cur.includes("-") ? base : `${maj}.${min}.${pat + 1}`;
}

// Write `version` into [workspace.package]. Only the dry run calls this — it
// stages the release state in the working tree and restores it afterwards. The
// real run lets `cargo ws version` do the bump so Cargo.lock gets refreshed too.
function setWorkspaceVersion(version) {
  const content = fs.readFileSync("Cargo.toml", "utf8");
  fs.writeFileSync(
    "Cargo.toml",
    content.replace(
      /^(\[workspace\.package\][\s\S]*?^version = )".*"$/m,
      `$1"${version}"`
    )
  );
}

// Re-pin the workspace-internal crates in [workspace.dependencies] to `version`.
//
// cargo-workspaces bumps [workspace.package] version but never touches
// [workspace.dependencies]: every intra-workspace dep is declared
// `workspace = true`, so there is nothing in the member manifests for it to
// rewrite. The published manifests carry whatever requirement was last written
// by hand.
//
// That is benign for a stable release — an older caret range still resolves to
// the new version — but it silently breaks pre-releases, because `^1.0.11`
// never matches `2.0.0-rc.1`. Without this, an rc of `ddk` would depend on the
// last *stable* ddk-manager rather than the rc published alongside it.
//
// Returns true if anything changed.
function pinWorkspaceDeps(version) {
  let inSection = false;
  let changed = 0;

  const updated = fs
    .readFileSync("Cargo.toml", "utf8")
    .split("\n")
    .map((line) => {
      if (line.startsWith("[")) {
        inSection = line.trim() === "[workspace.dependencies]";
        return line;
      }
      // Only the workspace's own crates carry a `path`; third-party pins stay.
      if (!inSection || !/\bpath\s*=/.test(line)) return line;
      const pinned = line.replace(
        /\bversion\s*=\s*"[^"]*"/,
        `version = "${version}"`
      );
      if (pinned !== line) changed++;
      return pinned;
    })
    .join("\n");

  if (!changed) return false;
  fs.writeFileSync("Cargo.toml", updated);
  console.log(`   Pinned ${changed} workspace dependencies to ${version}`);
  return true;
}

function checkCargoWs() {
  const wsVersion = run("cargo ws --version", { allowFailure: true });
  if (!wsVersion) {
    console.error(
      "❌ cargo-workspaces not found. Install it with:\n   cargo install cargo-workspaces"
    );
    process.exit(1);
  }
  console.log(`✅ ${wsVersion}`);
}

function checkGitStatus() {
  console.log("📋 Checking git status...");

  const status = run("git status --porcelain");
  if (status && !dryRun) {
    console.error(
      "❌ Git working directory is not clean. Please commit or stash your changes."
    );
    console.error("Uncommitted changes:");
    console.error(status);
    process.exit(1);
  } else if (status && dryRun) {
    console.warn("⚠️  Git working directory is not clean (ignored in dry run)");
  }

  const branch = run("git rev-parse --abbrev-ref HEAD");
  console.log(`✅ Git is clean on branch: ${branch}`);

  console.log("🔄 Fetching latest from origin...");
  run("git fetch origin master");

  const behind = run("git rev-list HEAD..origin/master --count");
  if (behind !== "0") {
    if (!dryRun) {
      console.error(
        `❌ Branch is ${behind} commits behind origin/master. Please pull latest changes.`
      );
      process.exit(1);
    }
    console.warn(
      `⚠️  Branch is ${behind} commits behind origin/master (ignored in dry run)`
    );
  } else {
    console.log("✅ Branch is up to date with origin/master");
  }
}

async function checkGitHubActions() {
  console.log("🔍 Checking GitHub Actions status...");

  try {
    const info = getRepoInfo();
    if (!info) {
      console.warn("⚠️  Could not parse GitHub repository from remote URL");
      return;
    }
    console.log(`Repository: ${info.owner}/${info.repo}`);

    const ghVersion = run("gh --version", { allowFailure: true });
    if (!ghVersion) {
      console.warn("⚠️  GitHub CLI (gh) not found. Skipping workflow check.");
      console.warn("   Install with: brew install gh");
      return;
    }

    const workflowRuns = run(
      `gh run list --branch master --limit 1 --json status,conclusion,headSha`
    );
    const runs = JSON.parse(workflowRuns);

    if (runs.length === 0) {
      console.warn("⚠️  No workflow runs found on master branch");
      return;
    }

    const latestRun = runs[0];
    // Gate on the master commit we're releasing from (origin/master), not local
    // HEAD — local HEAD is the release branch tip on a resume and has no run.
    const targetSha = run("git rev-parse origin/master");
    console.log(`Latest workflow SHA: ${latestRun.headSha.substring(0, 7)}`);

    // Make sure the run we're inspecting is actually for that commit, otherwise
    // a stale green run from an older commit would pass the gate.
    if (latestRun.headSha !== targetSha) {
      const msg = `Latest workflow ran against ${latestRun.headSha.substring(
        0,
        7
      )}, not origin/master ${targetSha.substring(0, 7)}.`;
      if (!dryRun) {
        console.error(`❌ ${msg} Wait for CI to run on the latest commit.`);
        process.exit(1);
      }
      console.warn(`⚠️  ${msg} (ignored in dry run)`);
      return;
    }

    if (latestRun.status === "completed" && latestRun.conclusion === "success") {
      console.log("✅ Latest GitHub Actions workflow succeeded");
    } else if (latestRun.status === "in_progress") {
      if (!dryRun) {
        console.error(
          "❌ GitHub Actions workflow is still in progress. Please wait for it to complete."
        );
        process.exit(1);
      }
      console.warn(
        "⚠️  GitHub Actions workflow is still in progress (ignored in dry run)"
      );
    } else {
      if (!dryRun) {
        console.error(
          `❌ Latest GitHub Actions workflow failed with status: ${latestRun.conclusion}`
        );
        process.exit(1);
      }
      console.warn(
        `⚠️  Latest GitHub Actions workflow failed with status: ${latestRun.conclusion} (ignored in dry run)`
      );
    }
  } catch (error) {
    console.warn("⚠️  Could not check GitHub Actions status:", error.message);
  }
}

// Find the commit the previous release ended on, to bound the notes range.
//
// `git describe --tags --abbrev=0` is wrong here. Release PRs are squash-merged,
// so the tag this script writes on the release branch points at a commit that
// never lands on master: v2.0.0-rc.2 is not an ancestor of master even though
// its squashed equivalent is. describe then walks silently back to the release
// before it, which is how the v2.0.0-rc.3 notes ended up spanning two releases.
//
// The release commits themselves do survive the squash, so anchor on the newest
// `chore: release v<x>` / `chore: pin workspace deps to v<x>` commit reachable
// from HEAD that is not part of the release being cut. Fall back to describe.
//
// Subjects are matched here rather than with `git log --grep`, which searches
// the whole message and so also matches merge commits that merely quote a
// release subject in their body.
function previousReleaseRef(version) {
  const releaseCommit = /^chore: (release|pin workspace deps to) v?\d/;
  // Match the version as a whole token, so cutting 2.0.0 does not mistake
  // "chore: release v2.0.0-rc.3" for its own commit.
  const isThisRelease = new RegExp(
    `v?${version.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?![\\w.-])`
  );

  const log = run("git log -n 200 --format='%H %s'", { allowFailure: true });

  for (const line of (log || "").split("\n").filter(Boolean)) {
    const sep = line.indexOf(" ");
    const subject = line.slice(sep + 1);
    if (!releaseCommit.test(subject) || isThisRelease.test(subject)) continue;
    return { ref: line.slice(0, sep), label: subject };
  }

  const tag = run("git describe --tags --abbrev=0", { allowFailure: true });
  return tag ? { ref: tag, label: tag } : null;
}

// Ask Claude for the notes. Returns the text, or throws with the reason.
//
// The prompt goes in on stdin rather than as `"$(cat file)"` so a long range
// cannot hit ARG_MAX, and stderr is folded into stdout so a failure reports
// what actually went wrong instead of "Claude returned no output".
function claudeNotes(prompt, version) {
  const promptFile = `/tmp/release-prompt-${version}.txt`;
  fs.writeFileSync(promptFile, prompt);
  try {
    // Measured at ~130s for a 12-commit range, so the ceiling is generous.
    let out;
    try {
      out = execSync(`claude -p < ${promptFile} 2>&1`, {
        encoding: "utf8",
        timeout: 600000,
        maxBuffer: 32 * 1024 * 1024,
      }).trim();
    } catch (error) {
      // stderr is folded into stdout above, so the real reason (an auth or
      // credit error, say) is in error.stdout rather than error.message.
      const detail = String(error.stdout || "").trim();
      throw new Error(detail ? detail.slice(0, 600) : error.message);
    }
    if (!out) throw new Error("claude -p produced no output");
    // A failing `claude -p` can still exit 0 with only a diagnostic on stdout.
    if (!out.startsWith("#")) {
      throw new Error(`claude -p did not return markdown notes: ${out.slice(0, 400)}`);
    }
    return out;
  } finally {
    fs.unlinkSync(promptFile);
  }
}

// Generate release notes for `version` into releases/<version>-RELEASE.md.
// Returns the notes content (used for the GitHub release body).
async function generateReleaseNotes(version) {
  console.log("\n📝 Generating release notes...");

  const releaseDir = "./releases";
  const releaseFile = path.join(releaseDir, `${version}-RELEASE.md`);

  if (fs.existsSync(releaseFile)) {
    console.log(`✅ Release notes already exist at ${releaseFile}`);
    return fs.readFileSync(releaseFile, "utf8");
  }

  if (!fs.existsSync(releaseDir) && !dryRun) {
    fs.mkdirSync(releaseDir, { recursive: true });
  }

  const previous = previousReleaseRef(version);
  console.log(`   Previous release: ${previous ? previous.label : "none"}`);

  const range = previous ? `${previous.ref}..HEAD` : "";
  // Subjects alone produce vague notes. The commit bodies in this repo carry the
  // rationale and the compatibility notes, so send those too, indented under
  // each subject and bounded so a long range cannot blow up the prompt.
  const commits = range
    ? run(`git log ${range} --format='- %h %s%n%w(0,4,4)%b'`).slice(0, 60000)
    : run("git log --oneline -20");
  const oneline = range
    ? run(`git log ${range} --oneline`)
    : run("git log --oneline -20");
  const gitDiff = range ? run(`git diff ${range} --stat`) : "";

  // An empty range is not an error: finalizing a release candidate to its base
  // version (2.0.0-rc.3 -> 2.0.0) is a promotion with no new commits.
  if (range && !oneline) {
    console.log(`   No new commits since ${previous.label} — promotion release`);
  } else {
    console.log(`   ${oneline.split("\n").length} commits in range`);
  }

  const prompt = `Generate professional release notes for version ${version} of the DLC DevKit (DDK) Rust workspace.

Here are the commits since the last release (${previous ? previous.label : "initial release"}), each with its full commit message body:
${commits || `(none — this release promotes the previous release candidate to ${version} with no code changes)`}

File changes summary:
${gitDiff}

The workspace contains these crates that are all being released with version ${version}:
- ddk-trie: Trie data structure for DLC
- ddk-messages: DLC message protocol implementation
- kormir: Oracle implementation
- ddk-dlc: Core DLC functionality
- ddk-manager: DLC management and coordination
- ddk: Main DLC DevKit library
- ddk-payouts: Payout calculation utilities
- ddk-node: DLC node implementation

Please create release notes with:
1. A brief summary of the release
2. Breaking changes (if any, look for BREAKING in commits or major API changes)
3. Security fixes, when a commit body describes one
4. New features (commits starting with feat:)
5. Bug fixes (commits starting with fix:)
6. Other notable changes
7. An upgrading section whenever a commit body describes on-disk or wire
   compatibility, saying explicitly what keeps working
8. Installation instructions showing how to add ddk = "${version}" to Cargo.toml

Write the specifics the commit bodies give you — the affected type and function
names, what changed about them, and why. Do not just restate the commit
subjects. Cite the short SHA for each entry. Format as clean markdown suitable
for a GitHub release.

Output the release notes and nothing else: the very first line must be the
top-level heading, and the last line must be the last line of the notes. No
preamble, no commentary addressed to whoever ran this, no trailing questions —
the output is written verbatim into the GitHub release body.`;

  if (dryRun) {
    console.log("   [DRY RUN] Would generate release notes using Claude");
    return `# Release v${version}\n\n[DRY RUN - notes generated here]\n`;
  }

  console.log("   Using Claude to generate release notes (up to 10 min)...");
  let lastError;
  for (let attempt = 1; attempt <= 2; attempt++) {
    try {
      const notes = claudeNotes(prompt, version);
      fs.writeFileSync(releaseFile, notes);
      console.log(`✅ Release notes written to ${releaseFile}`);
      return notes;
    } catch (error) {
      lastError = error;
      if (attempt === 1) console.warn(`⚠️  Attempt 1 failed; retrying...`);
    }
  }

  // Never ship the bare commit list without being told to. The v2.0.0-rc.3
  // notes went out as a raw `git log` dump because this fell through silently,
  // and by then the crates were already published.
  console.error(`\n❌ Could not generate release notes: ${lastError.message}`);
  console.error(
    "   If this mentions credit or authentication, ANTHROPIC_API_KEY is set in\n" +
      "   this shell and takes precedence over the claude.ai login. Re-run with\n" +
      "   `env -u ANTHROPIC_API_KEY node release.js ...`, or write\n" +
      `   ${releaseFile} by hand and re-run — existing notes are reused as-is.`
  );

  const proceed =
    skipConfirm ||
    (await ask(
      "\n❓ Continue anyway with a basic commit-list template? [y/N] "
    ));
  if (!proceed) {
    console.log("Aborted before publishing. Nothing was released.");
    process.exit(1);
  }

  let content = `# Release v${version}\n\n`;
  content += `Released: ${new Date().toISOString().split("T")[0]}\n\n`;
  content += `## 📥 Installation\n\n\`\`\`toml\nddk = "${version}"\n\`\`\`\n\n`;
  content += `## Commits\n\n\`\`\`\n${oneline}\n\`\`\`\n`;
  fs.writeFileSync(releaseFile, content);
  console.log(`✅ Basic release notes written to ${releaseFile}`);
  return content;
}

// Report what the dry run actually did.
//
// cargo-workspaces reports a crate it could not publish as `warn publish failed
// <crate>` and still exits 0 with `info success ok`, so the dry run's exit code
// on its own is a false green.
//
// One class of failure here is expected and unavoidable. `pinWorkspaceDeps`
// points every intra-workspace dependency at the version being released, and
// cargo re-resolves a packaged crate against the registry, where the sibling
// crates do not exist yet — so a pre-release dry run cannot verify any crate
// that depends on another workspace crate. The real run does not hit this
// because it publishes in dependency order. Anything else is a genuine failure.
function reportDryRunResult(output, version) {
  const failed = [...output.matchAll(/^warn publish failed (\S+)/gm)].map(
    (m) => m[1]
  );
  if (!failed.length) {
    console.log("\n✅ Dry run packaged and verified every crate");
    return;
  }

  const escaped = version.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const unresolvedSibling = new RegExp(
    `failed to select a version for the requirement \`[\\w-]+ = "\\^?${escaped}"`
  );

  // cargo-workspaces prints an `info checking <crate>` banner before each
  // crate, so each chunk holds one crate's output and its own failure reason.
  const sections = output.split(/^info checking /m).slice(1);
  const expected = [];
  const real = [];
  for (const section of sections) {
    const crate = section.slice(0, section.indexOf("\n")).trim();
    if (!/^warn publish failed /m.test(section)) continue;
    (unresolvedSibling.test(section) ? expected : real).push(crate);
  }
  // If the banners ever change shape, fall back to judging the whole output.
  if (!expected.length && !real.length) {
    (unresolvedSibling.test(output) ? expected : real).push(...failed);
  }

  if (expected.length) {
    console.log(
      `\n⚠️  Not verifiable in a dry run: ${expected.join(", ")}`
    );
    console.log(
      `   These depend on workspace crates pinned to ${version}, which is not on\n` +
        "   crates.io yet. The real run publishes in dependency order, so each\n" +
        "   sibling is on the index by the time the next crate is packaged."
    );
  }

  if (real.length) {
    console.error(
      `\n❌ Dry run failed to publish: ${real.join(", ")}\n` +
        "   Scroll up for the cargo error. cargo-workspaces exits 0 on these, so\n" +
        "   without this check the dry run would have looked like a success."
    );
    process.exit(1);
  }
}

// Confirm every publishable crate actually landed on crates.io.
//
// cargo-workspaces reports a crate it couldn't publish as `warn publish failed
// <crate>` and still exits 0 with `info success ok`. Without this check a
// partial publish would sail straight into tagging, pushing, and cutting a
// GitHub release for a version that isn't fully on the registry.
function verifyPublished(version) {
  waitForPackageCache();
  const meta = JSON.parse(run("cargo metadata --no-deps --format-version 1"));
  // `publish` is null when unrestricted and [] for `publish = false`.
  const crates = meta.packages
    .filter((p) => p.publish === null || p.publish.length > 0)
    .map((p) => p.name)
    .sort();

  console.log(`\n🔎 Verifying ${crates.length} crates on crates.io...`);
  const missing = [];
  for (const name of crates) {
    let found = false;
    // The index lags the upload by a moment, so give each crate a few tries.
    for (let attempt = 0; attempt < 5 && !found; attempt++) {
      if (attempt > 0) run("sleep 5");
      // crates.io rejects requests without a User-Agent with a 403.
      const code = run(
        `curl -s -A 'dlcdevkit-release-script' -o /dev/null -w '%{http_code}' ` +
          `https://crates.io/api/v1/crates/${name}/${version}`,
        { allowFailure: true }
      );
      found = code === "200";
    }
    console.log(`   ${found ? "✅" : "❌"} ${name} ${version}`);
    if (!found) missing.push(name);
  }

  if (missing.length) {
    console.error(
      `\n❌ Not on crates.io: ${missing.join(", ")}\n` +
        `   Stopping before the tag, push, and GitHub release. Re-run the same\n` +
        `   command to resume — cargo-workspaces skips crates already published.`
    );
    process.exit(1);
  }
  console.log("✅ All crates verified on crates.io");
}

async function createGitHubRelease(version, releaseNotes) {
  console.log("\n🚀 Creating GitHub release...");

  const ghVersion = run("gh --version", { allowFailure: true });
  if (!ghVersion) {
    console.warn("⚠️  GitHub CLI (gh) not found. Skipping GitHub release.");
    return;
  }

  const info = getRepoInfo();
  const slug = info ? `${info.owner}/${info.repo}` : "<owner>/<repo>";

  try {
    const tempFile = `/tmp/release-notes-${version}.md`;
    fs.writeFileSync(tempFile, releaseNotes);
    // Pre-releases must not displace the last stable release as "Latest".
    const prerelease = version.includes("-") ? " --prerelease" : "";
    run(
      `gh release create v${version} --title "v${version}"${prerelease} --notes-file ${tempFile}`
    );
    fs.unlinkSync(tempFile);
    console.log(`✅ GitHub release v${version} created`);
    console.log(`   View at: https://github.com/${slug}/releases/tag/v${version}`);
  } catch (error) {
    console.warn(`⚠️  Failed to create GitHub release: ${error.message}`);
    console.log(`   Create it manually: https://github.com/${slug}/releases/new`);
  }
}

// ---------------------------------------------------------------------------
// Main release process
// ---------------------------------------------------------------------------
async function release() {
  // Step 0: tooling + working-tree preflight (cheap, fail fast).
  checkCargoWs();

  const cur = currentVersion();
  const version = explicitVersion || nextVersion(cur, bumpKind);
  const releaseBranch = `release-${version}`;

  console.log(
    `\n🚀 Release: ${cur} → ${version} (${
      explicitVersion ? "explicit" : bumpKind
    })${dryRun ? " — DRY RUN" : ""}\n`
  );

  // Step 1: git + CI gates.
  checkGitStatus();
  await checkGitHubActions();

  // Step 2: confirm before doing anything irreversible.
  if (!dryRun && !skipConfirm) {
    const confirmed = await ask(
      `\n❓ Publish all crates as v${version} to crates.io? [y/N] `
    );
    if (!confirmed) {
      console.log("Aborted.");
      process.exit(0);
    }
  }

  // Step 3: dry run validates the whole pipeline without mutating anything.
  if (dryRun) {
    console.log("\n📝 Generating release notes (preview)...");
    await generateReleaseNotes(version);

    console.log("\n📦 Validating publish via cargo-workspaces (--dry-run)...");
    // Stage the exact manifest state the real run publishes — bumped version
    // *and* re-pinned intra-workspace deps — so the dry run validates what will
    // actually go to crates.io, then put Cargo.toml back byte-for-byte.
    const original = fs.readFileSync("Cargo.toml", "utf8");
    let output = "";
    try {
      setWorkspaceVersion(version);
      pinWorkspaceDeps(version);
      output = runCargoLive(
        `cargo ws publish --publish-as-is --allow-branch '*' ` +
          `--no-git-tag --no-git-push --dry-run --allow-dirty -y`
      );
    } finally {
      fs.writeFileSync("Cargo.toml", original);
      console.log("   Restored Cargo.toml (Cargo.lock may have been refreshed)");
    }
    reportDryRunResult(output, version);

    console.log("\n🎉 Dry run complete. To perform the real release:");
    console.log(
      `   node release.js ${explicitVersion ? version : `--${bumpKind}`}`
    );
    return;
  }

  // Step 4: create (or resume) the release branch.
  const branchExists = run(`git rev-parse --verify ${releaseBranch} 2>/dev/null`, {
    allowFailure: true,
  });
  if (branchExists) {
    run(`git checkout ${releaseBranch}`);
    console.log(`✅ Reusing existing branch ${releaseBranch}`);
  } else {
    run(`git checkout -b ${releaseBranch}`);
    console.log(`✅ Created branch ${releaseBranch}`);
  }

  // Step 5: generate release notes. The `releases/` dir is gitignored — the
  // notes live on the GitHub release page (see Step 10), not in the repo — so
  // there's nothing to commit, and the ignored file doesn't dirty the working
  // tree before cargo-workspaces runs.
  const releaseNotes = await generateReleaseNotes(version);

  // Step 6: bump + publish in dependency order via cargo-workspaces.
  // It derives the publish order from the dependency graph and skips crates
  // already on crates.io, so a re-run safely resumes a partial publish.
  //
  // If the branch is already bumped (resume after a mid-publish failure),
  // publish the existing versions as-is instead of re-versioning.
  const alreadyBumped = currentVersion() === version;
  if (alreadyBumped) {
    console.log(`\n📦 Versions already at ${version} — skipping the bump`);
  } else {
    console.log("\n📦 Bumping crate versions via cargo-workspaces...");
    runCargoLive(
      `cargo ws version custom ${version} --force '*' ` +
        `--allow-branch 'release-*' --no-git-tag --no-git-push -y ` +
        `-m "chore: release v%v"`
    );
  }

  // Pin [workspace.dependencies] to the version just bumped to. This has to
  // happen *after* the bump — pinning first would leave the manifest asking for
  // a version the local path crates don't yet have, which `cargo metadata`
  // refuses to resolve. Idempotent, so a resumed run is a no-op here.
  if (pinWorkspaceDeps(version)) {
    run(`git commit -m "chore: pin workspace deps to v${version}" -- Cargo.toml`);
    console.log("✅ Workspace dependency pins committed");
  }

  console.log("\n📦 Publishing crates to crates.io via cargo-workspaces...");
  runCargoLive(
    `cargo ws publish --publish-as-is --allow-branch 'release-*' ` +
      `--no-git-tag --no-git-push -y`
  );
  console.log("✅ All crates published");
  verifyPublished(version);

  // Step 7: tag the release commit (cargo ws tagging is disabled above so this
  // is the single source of truth and is idempotent across resumes).
  run(`git tag -a v${version} -m "Release v${version}"`, {
    allowFailure: true,
  });

  // Step 8: push branch + tag.
  console.log("\n📤 Pushing release branch and tag to origin...");
  run(`git push -u origin ${releaseBranch}`);
  run(`git push origin v${version}`);
  console.log("✅ Branch and tag pushed");

  // Step 9: open the release PR.
  const info = getRepoInfo();
  const slug = info ? `${info.owner}/${info.repo}` : "<owner>/<repo>";
  console.log("📝 Creating pull request...");
  const prBodyFile = `/tmp/release-pr-body-${version}.md`;
  // Inline the notes rather than pointing at releases/<version>-RELEASE.md:
  // that directory is gitignored, so the file a reviewer is sent to look for is
  // not in the repo. The GitHub release carries the same text.
  fs.writeFileSync(
    prBodyFile,
    `Bumps all crate versions to ${version} and publishes them to crates.io.\n\n` +
      `Tag: [v${version}](https://github.com/${slug}/releases/tag/v${version})\n\n` +
      `---\n\n${releaseNotes}\n`
  );
  try {
    const prUrl = run(
      `gh pr create --title "chore: release ${version}" --body-file ${prBodyFile} --base master --head ${releaseBranch}`
    );
    console.log(`✅ Pull request created: ${prUrl}`);
  } catch (error) {
    console.warn("⚠️  Could not create PR automatically:", error.message);
    console.log(
      `   Create it manually: https://github.com/${slug}/compare/master...${releaseBranch}`
    );
  }
  fs.unlinkSync(prBodyFile);

  // Step 10: GitHub release.
  await createGitHubRelease(version, releaseNotes);

  // Step 11: back to master. cargo's per-crate publish verification re-resolves
  // dependencies and can rewrite Cargo.lock with incidental transitive drift
  // (e.g. a patch bump of a transitive dep) *after* cargo-workspaces made the
  // release commit. That dirties the working tree and would abort the branch
  // switch. The drift isn't part of the release — the published crates and the
  // tag don't include it, and it re-resolves on the next build — so discard it.
  // Everything important is already published/pushed, so keep this cleanup
  // best-effort rather than failing the whole run on it.
  run("git checkout -- Cargo.lock", { allowFailure: true });
  if (run("git checkout master", { allowFailure: true }) === null) {
    console.warn(
      "⚠️  Could not switch back to master — the working tree still has local " +
        "changes. The release itself is complete; run `git checkout master` " +
        "manually after handling them."
    );
  }

  console.log("\n🎉 Release complete!");
  console.log("   - All crates published to crates.io");
  console.log(`   - Tag v${version} created and pushed`);
  console.log("   - Release PR opened");
  console.log("   - GitHub release created");
  console.log("\n⚠️  Next step: review and merge the release PR.");
}

release().catch((error) => {
  console.error("❌ Release failed:", error);
  process.exit(1);
});
