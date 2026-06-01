#!/usr/bin/env node

const { execSync } = require("child_process");
const fs = require("fs");
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

// Stream a command's output straight to the terminal (for long-running,
// chatty commands like `cargo ws publish` where live progress matters).
function runLive(command, options = {}) {
  if (options.dryRun && dryRun) {
    console.log(`   [DRY RUN] Would execute: ${command}`);
    return;
  }
  execSync(command, { stdio: "inherit", ...options });
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

// Compute the next version from a bump keyword. Drops any pre-release suffix.
function nextVersion(cur, kind) {
  const [maj, min, pat] = cur.split("-")[0].split(".").map(Number);
  if (kind === "major") return `${maj + 1}.0.0`;
  if (kind === "minor") return `${maj}.${min + 1}.0`;
  return `${maj}.${min}.${pat + 1}`;
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

// Generate release notes for `version` into releases/<version>-RELEASE.md.
// Returns the notes content (used for the GitHub release body).
function generateReleaseNotes(version) {
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

  const latestTag =
    run("git describe --tags --abbrev=0", { allowFailure: true }) || "";
  console.log(`   Latest tag: ${latestTag || "none"}`);

  const commits = latestTag
    ? run(`git log ${latestTag}..HEAD --oneline`)
    : run("git log --oneline -20");
  const gitDiff = latestTag ? run(`git diff ${latestTag}..HEAD --stat`) : "";

  const prompt = `Generate professional release notes for version ${version} of the DLC DevKit (DDK) Rust workspace.

Here are the commits since the last release (${latestTag || "initial release"}):
${commits}

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
3. New features (commits starting with feat:)
4. Bug fixes (commits starting with fix:)
5. Other notable changes
6. Installation instructions showing how to add ddk = "${version}" to Cargo.toml

Format as clean markdown suitable for a GitHub release. Be concise but informative.`;

  if (dryRun) {
    console.log("   [DRY RUN] Would generate release notes using Claude");
    return `# Release v${version}\n\n[DRY RUN - notes generated here]\n`;
  }

  try {
    console.log("   Using Claude to generate release notes...");
    const tempPromptFile = `/tmp/release-prompt-${version}.txt`;
    fs.writeFileSync(tempPromptFile, prompt);
    const claudeOutput = run(`claude -p "$(cat ${tempPromptFile})"`, {
      allowFailure: true,
      timeout: 60000,
    });
    fs.unlinkSync(tempPromptFile);

    if (!claudeOutput) throw new Error("Claude returned no output");

    fs.writeFileSync(releaseFile, claudeOutput);
    console.log(`✅ Release notes written to ${releaseFile}`);
    return claudeOutput;
  } catch (error) {
    console.warn(
      `⚠️  Claude release notes failed (${error.message}); using basic template.`
    );
    let content = `# Release v${version}\n\n`;
    content += `Released: ${new Date().toISOString().split("T")[0]}\n\n`;
    content += `## 📥 Installation\n\n\`\`\`toml\nddk = "${version}"\n\`\`\`\n\n`;
    content += `## Commits\n\n\`\`\`\n${commits}\n\`\`\`\n`;
    fs.writeFileSync(releaseFile, content);
    console.log(`✅ Basic release notes written to ${releaseFile}`);
    return content;
  }
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
    run(
      `gh release create v${version} --title "v${version}" --notes-file ${tempFile}`
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
    const rl = readline.createInterface({
      input: process.stdin,
      output: process.stdout,
    });
    const answer = await rl.question(
      `\n❓ Publish all crates as v${version} to crates.io? [y/N] `
    );
    rl.close();
    if (answer.trim().toLowerCase() !== "y") {
      console.log("Aborted.");
      process.exit(0);
    }
  }

  // Step 3: dry run validates the whole pipeline without mutating anything.
  if (dryRun) {
    console.log("\n📝 Generating release notes (preview)...");
    generateReleaseNotes(version);

    console.log("\n📦 Validating publish via cargo-workspaces (--dry-run)...");
    runLive(
      `cargo ws publish custom ${version} --force '*' --allow-branch '*' ` +
        `--no-git-tag --no-git-push --dry-run --allow-dirty -y`
    );

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
  const releaseNotes = generateReleaseNotes(version);

  // Step 6: bump + publish in dependency order via cargo-workspaces.
  // It derives the publish order from the dependency graph and skips crates
  // already on crates.io, so a re-run safely resumes a partial publish.
  //
  // If the branch is already bumped (resume after a mid-publish failure),
  // publish the existing versions as-is instead of re-versioning.
  const alreadyBumped = currentVersion() === version;
  console.log("\n📦 Publishing crates to crates.io via cargo-workspaces...");
  if (alreadyBumped) {
    console.log(`   (versions already at ${version} — publishing as-is)`);
    runLive(
      `cargo ws publish --publish-as-is --allow-branch 'release-*' ` +
        `--no-git-tag --no-git-push -y`
    );
  } else {
    runLive(
      `cargo ws publish custom ${version} --force '*' ` +
        `--allow-branch 'release-*' --no-git-tag --no-git-push -y ` +
        `-m "chore: release v%v"`
    );
  }
  console.log("✅ All crates published");

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
  fs.writeFileSync(
    prBodyFile,
    `Release version ${version}\n\n## Changes\n- Bumped all crate versions to ${version}\n- Published crates to crates.io\n\n## Release Notes\nSee releases/${version}-RELEASE.md\n`
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

  // Step 11: back to master.
  run("git checkout master");

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
