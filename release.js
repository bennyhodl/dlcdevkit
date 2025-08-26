#!/usr/bin/env node

const { execSync } = require("child_process");
const fs = require("fs");
const path = require("path");

// Parse command line arguments
const args = process.argv.slice(2);
const dryRun = args.includes("--dry");
const versionArg = args.find((arg) => !arg.startsWith("--"));

if (!versionArg) {
  console.error("Usage: node release.js <version> [--dry]");
  console.error("Example: node release.js 0.22.0");
  console.error("         node release.js 0.22.0 --dry");
  process.exit(1);
}

const version = versionArg;

// Validate version format
if (!/^\d+\.\d+\.\d+(-.*)?$/.test(version)) {
  console.error(
    "Invalid version format. Use semantic versioning (e.g., 1.2.3 or 1.2.3-beta.1)"
  );
  process.exit(1);
}

if (dryRun) {
  console.log("🔍 DRY RUN MODE - No changes will be committed or published\n");
}

// Helper function to run commands
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

// Helper function to update version in Cargo.toml
function updateCargoVersion(cratePath, newVersion, isDryRun = false) {
  const cargoPath = path.join(cratePath, "Cargo.toml");
  let content = fs.readFileSync(cargoPath, "utf8");
  const originalContent = content;

  // Update package version
  content = content.replace(/^version = ".*"$/m, `version = "${newVersion}"`);

  // Update workspace dependencies to use the new version
  const crateNames = [
    "ddk",
    "ddk-dlc",
    "ddk-messages",
    "ddk-trie",
    "ddk-manager",
    "ddk-payouts",
    "kormir",
    "payouts",
    "ddk-node",
  ];

  crateNames.forEach((crateName) => {
    // Update path dependencies with version
    const pathDepRegex = new RegExp(
      `^${crateName} = \\{ version = "[^"]*"(.*path = "\\.\\./[^"]*".*)\\}$`,
      "gm"
    );
    content = content.replace(
      pathDepRegex,
      `${crateName} = { version = "${newVersion}"$1}`
    );

    // Also handle the ddk-* prefixed versions
    if (crateName.startsWith("ddk-")) {
      const regex = new RegExp(
        `^${crateName.replace(
          "ddk-",
          "ddk-"
        )} = \\{ version = "[^"]*"(.*path = "\\.\\./[^"]*".*)\\}$`,
        "gm"
      );
      content = content.replace(
        regex,
        `${crateName} = { version = "${newVersion}"$1}`
      );
    }
  });

  if (isDryRun) {
    if (content !== originalContent) {
      console.log(`   [DRY RUN] Would update ${cargoPath}`);
      // Show what would change
      const oldVersion = originalContent.match(/^version = "(.*)"$/m)?.[1];
      if (oldVersion && oldVersion !== newVersion) {
        console.log(`            Version: ${oldVersion} → ${newVersion}`);
      }
    }
  } else {
    fs.writeFileSync(cargoPath, content);
  }
}

// Check git status
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

  // Fetch latest from origin
  console.log("🔄 Fetching latest from origin...");
  run("git fetch origin master");

  // Check if we're behind origin/master
  const behind = run("git rev-list HEAD..origin/master --count");
  if (behind !== "0") {
    if (!dryRun) {
      console.error(
        `❌ Branch is ${behind} commits behind origin/master. Please pull latest changes.`
      );
      process.exit(1);
    } else {
      console.warn(
        `⚠️  Branch is ${behind} commits behind origin/master (ignored in dry run)`
      );
    }
  } else {
    console.log("✅ Branch is up to date with origin/master");
  }
}

// Check GitHub Actions status
async function checkGitHubActions() {
  console.log("🔍 Checking GitHub Actions status...");

  try {
    // Get the repository info from git remote
    const remoteUrl = run("git remote get-url origin");
    const match = remoteUrl.match(/github\.com[:/]([^/]+)\/([^.]+)/);

    if (!match) {
      console.warn("⚠️  Could not parse GitHub repository from remote URL");
      return;
    }

    const [, owner, repo] = match;
    console.log(`Repository: ${owner}/${repo}`);

    // Check if gh CLI is available
    const ghVersion = run("gh --version", { allowFailure: true });
    if (!ghVersion) {
      console.warn("⚠️  GitHub CLI (gh) not found. Skipping workflow check.");
      console.warn("   Install with: brew install gh");
      return;
    }

    // Get the latest workflow run on master
    const workflowRuns = run(
      `gh run list --branch master --limit 1 --json status,conclusion,headSha`
    );
    const runs = JSON.parse(workflowRuns);

    if (runs.length === 0) {
      console.warn("⚠️  No workflow runs found on master branch");
      return;
    }

    const latestRun = runs[0];
    console.log(`Latest workflow SHA: ${latestRun.headSha.substring(0, 7)}`);

    if (
      latestRun.status === "completed" &&
      latestRun.conclusion === "success"
    ) {
      console.log("✅ Latest GitHub Actions workflow succeeded");
    } else if (latestRun.status === "in_progress") {
      if (!dryRun) {
        console.error(
          "❌ GitHub Actions workflow is still in progress. Please wait for it to complete."
        );
        process.exit(1);
      } else {
        console.warn(
          "⚠️  GitHub Actions workflow is still in progress (ignored in dry run)"
        );
      }
    } else {
      if (!dryRun) {
        console.error(
          `❌ Latest GitHub Actions workflow failed with status: ${latestRun.conclusion}`
        );
        process.exit(1);
      } else {
        console.warn(
          `⚠️  Latest GitHub Actions workflow failed with status: ${latestRun.conclusion} (ignored in dry run)`
        );
      }
    }
  } catch (error) {
    console.warn("⚠️  Could not check GitHub Actions status:", error.message);
  }
}

// Generate release notes
function generateReleaseNotes() {
  console.log("\n📝 Generating release notes...");

  const releaseDir = "./releases";
  const releaseFile = path.join(releaseDir, `${version}-RELEASE.md`);

  // Check if release notes already exist
  if (fs.existsSync(releaseFile)) {
    console.log(`✅ Release notes already exist at ${releaseFile}`);
    return fs.readFileSync(releaseFile, "utf8");
  }

  // Create releases directory if it doesn't exist
  if (!fs.existsSync(releaseDir)) {
    if (!dryRun) {
      fs.mkdirSync(releaseDir, { recursive: true });
    } else {
      console.log("   [DRY RUN] Would create releases directory");
    }
  }

  // Get the latest tag
  const latestTag =
    run("git describe --tags --abbrev=0", { allowFailure: true }) || "";
  console.log(`   Latest tag: ${latestTag || "none"}`);

  // Get commit history since last tag
  let commits = "";
  if (latestTag) {
    commits = run(`git log ${latestTag}..HEAD --oneline`);
  } else {
    // If no tags, get last 20 commits
    commits = run("git log --oneline -20");
  }

  // Get detailed git diff
  let gitDiff = "";
  if (latestTag) {
    gitDiff = run(`git diff ${latestTag}..HEAD --stat`);
  }

  // Prepare the prompt for Claude
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
- payouts: Payout calculation utilities
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
    console.log("   Prompt preview:", prompt.substring(0, 200) + "...");

    // Return a dummy content for dry run
    const dummyContent = `# Release v${version}\n\n[DRY RUN - Release notes would be generated here using Claude]\n`;
    return dummyContent;
  }

  try {
    // Use Claude to generate release notes
    console.log("   Using Claude to generate release notes...");

    // Write prompt to temporary file to avoid shell escaping issues
    const tempPromptFile = `/tmp/release-prompt-${Date.now()}.txt`;
    fs.writeFileSync(tempPromptFile, prompt);

    // Call Claude with the prompt
    const claudeOutput = run(`claude -p "$(cat ${tempPromptFile})"`, {
      allowFailure: false,
      timeout: 60000, // 60 second timeout for Claude
    });

    // Clean up temp file
    fs.unlinkSync(tempPromptFile);

    // Write release notes
    fs.writeFileSync(releaseFile, claudeOutput);
    console.log(`✅ Release notes written to ${releaseFile}`);

    return claudeOutput;
  } catch (error) {
    console.error(
      "❌ Failed to generate release notes with Claude:",
      error.message
    );
    console.log("   Falling back to basic template...");

    // Fallback to basic template if Claude fails
    let content = `# Release v${version}\n\n`;
    content += `Released: ${new Date().toISOString().split("T")[0]}\n\n`;
    content += `## 📦 Published Crates\n\n`;
    content += `All crates updated to version ${version}:\n\n`;
    content += `- \`ddk-trie\`\n`;
    content += `- \`ddk-messages\`\n`;
    content += `- \`kormir\`\n`;
    content += `- \`ddk-dlc\`\n`;
    content += `- \`ddk-manager\`\n`;
    content += `- \`ddk\`\n`;
    content += `- \`payouts\`\n`;
    content += `- \`ddk-node\`\n\n`;
    content += `## 📥 Installation\n\n`;
    content += `\`\`\`toml\n`;
    content += `# Add to your Cargo.toml\n`;
    content += `ddk = "${version}"\n`;
    content += `\`\`\`\n\n`;
    content += `## Commits\n\n`;
    content += `\`\`\`\n${commits}\n\`\`\`\n`;

    if (!dryRun) {
      fs.writeFileSync(releaseFile, content);
      console.log(`✅ Basic release notes written to ${releaseFile}`);
    }

    return content;
  }
}

// Create GitHub release
async function createGitHubRelease(releaseNotes) {
  console.log("\n🚀 Creating GitHub release...");

  // Check if gh CLI is available
  const ghVersion = run("gh --version", { allowFailure: true });
  if (!ghVersion) {
    console.warn(
      "⚠️  GitHub CLI (gh) not found. Skipping GitHub release creation."
    );
    console.warn("   Install with: brew install gh");
    return;
  }

  if (dryRun) {
    console.log(`   [DRY RUN] Would create GitHub release for tag v${version}`);
    return;
  }

  try {
    // Create the release using gh CLI
    const releaseCmd = `gh release create v${version} --title "v${version}" --notes "${releaseNotes
      .replace(/"/g, '\\"')
      .replace(/\n/g, "\\n")}"`;

    // For very long release notes, use a file instead
    const tempFile = `/tmp/release-notes-${version}.md`;
    fs.writeFileSync(tempFile, releaseNotes);

    run(
      `gh release create v${version} --title "v${version}" --notes-file ${tempFile}`
    );

    // Clean up temp file
    fs.unlinkSync(tempFile);

    console.log(`✅ GitHub release v${version} created successfully`);
    console.log(
      `   View at: https://github.com/<owner>/<repo>/releases/tag/v${version}`
    );
  } catch (error) {
    console.warn(`⚠️  Failed to create GitHub release: ${error.message}`);
    console.log(
      "   You can create it manually at: https://github.com/<owner>/<repo>/releases/new"
    );
  }
}

// Define crate dependencies and publish order
const crateOrder = [
  // Level 0: No internal dependencies
  { name: "dlc", path: "./dlc", package: "ddk-dlc" },
  
  // Level 1: Depends on level 0
  { name: "dlc-trie", path: "./dlc-trie", package: "ddk-trie" },
  { name: "dlc-messages", path: "./dlc-messages", package: "ddk-messages" },
  { name: "kormir", path: "./kormir", package: "kormir" },
  
  // Level 2: Depends on levels 0 and 1
  { name: "ddk-manager", path: "./ddk-manager", package: "ddk-manager" },
  
  // Level 3: Depends on level 2
  { name: "ddk", path: "./ddk", package: "ddk" },
  { name: "payouts", path: "./payouts", package: "ddk-payouts" },
  
  // Level 4: Depends on level 3
  { name: "ddk-node", path: "./ddk-node", package: "ddk-node" },
];

// Main release process
async function release() {
  console.log(
    `🚀 Starting release process for version ${version}${
      dryRun ? " (DRY RUN)" : ""
    }\n`
  );

  // Step 1: Check git status
  checkGitStatus();

  // Step 2: Check GitHub Actions
  await checkGitHubActions();

  // Step 3: Generate release notes (do this before updating versions)
  const releaseNotes = generateReleaseNotes();

  // Step 4: Update versions
  console.log(
    `\n📝 ${dryRun ? "Checking" : "Updating"} versions to ${version}...`
  );

  for (const crate of crateOrder) {
    console.log(`  ${dryRun ? "Checking" : "Updating"} ${crate.name}...`);
    updateCargoVersion(crate.path, version, dryRun);
  }

  console.log(`✅ All versions ${dryRun ? "checked" : "updated"}`);

  // Step 5: Build all crates to verify
  console.log("\n🔨 Building all crates to verify changes...");
  if (!dryRun) {
    run("cargo build --all");
    console.log("✅ Build successful");
  } else {
    console.log("   [DRY RUN] Would run: cargo build --all");
  }

  // Step 6: Commit version changes
  console.log("\n📝 Committing version changes...");
  run("git add .", { dryRun: true });
  run(`git commit -m "chore: release v${version}"`, { dryRun: true });
  if (!dryRun) {
    console.log("✅ Changes committed");
  }

  // Step 7: Create git tag
  console.log("\n🏷️  Creating git tag...");
  run(`git tag -a v${version} -m "Release v${version}"`, { dryRun: true });
  if (!dryRun) {
    console.log(`✅ Tag v${version} created`);
  }

  // Step 8: Publish crates in order
  console.log("\n📦 Publishing crates to crates.io...");
  console.log("   Publishing in dependency order:\n");

  for (const crate of crateOrder) {
    console.log(`📤 ${dryRun ? "Checking" : "Publishing"} ${crate.package}...`);

    if (dryRun) {
      // In dry run, just do cargo publish --dry-run
      console.log(
        `   [DRY RUN] Would publish ${crate.package} from ${crate.path}`
      );
      try {
        run(`cargo publish --dry-run`, { cwd: crate.path });
        console.log(`   ✅ ${crate.package} is ready to publish`);
      } catch (error) {
        console.error(`   ❌ ${crate.package} would fail to publish`);
        console.error(`      ${error.message.split("\n")[0]}`);
      }
    } else {
      try {
        // Dry run first
        run(`cargo publish --dry-run`, { cwd: crate.path });

        // Actual publish
        const publishOutput = run(`cargo publish`, {
          cwd: crate.path,
          allowFailure: true,
        });

        if (publishOutput === null) {
          // Check if it's already published
          const checkOutput = run(`cargo search ${crate.package} --limit 1`);
          if (checkOutput.includes(`${crate.package} = "${version}"`)) {
            console.log(`   ✅ ${crate.package} v${version} already published`);
          } else {
            console.error(`   ❌ Failed to publish ${crate.package}`);
            console.error("   You may need to retry or publish manually");
            continue;
          }
        } else {
          console.log(`   ✅ ${crate.package} published successfully`);
        }

        // Wait a bit between publishes to allow crates.io to update
        if (crate !== crateOrder[crateOrder.length - 1]) {
          console.log("   ⏳ Waiting 30 seconds for crates.io to update...");
          await new Promise((resolve) => setTimeout(resolve, 30000));
        }
      } catch (error) {
        console.error(
          `   ❌ Error publishing ${crate.package}: ${error.message}`
        );
        console.error("   You may need to retry or publish manually");
      }
    }
  }

  // Step 9: Create PR and GitHub release
  if (!dryRun) {
    console.log("\n🔄 Creating release pull request...");
    
    // Create a new branch for the release
    const releaseBranch = `release-${version}`;
    run(`git checkout -b ${releaseBranch}`);
    console.log(`✅ Created branch ${releaseBranch}`);
    
    // Push the branch
    console.log("📤 Pushing release branch to origin...");
    run(`git push -u origin ${releaseBranch}`);
    console.log("✅ Branch pushed");
    
    // Push tags separately
    console.log("🏷️  Pushing tags...");
    run("git push origin --tags");
    console.log("✅ Tags pushed");
    
    // Create PR using gh CLI
    console.log("📝 Creating pull request...");
    try {
      const prUrl = run(
        `gh pr create --title "chore: release ${version}" --body "Release version ${version}\n\n## Changes\n- Updated all crate versions to ${version}\n- Published crates to crates.io\n\n## Release Notes\nSee releases/${version}-RELEASE.md" --base master --head ${releaseBranch}`
      );
      console.log(`✅ Pull request created: ${prUrl}`);
    } catch (error) {
      console.warn("⚠️  Could not create PR automatically:", error.message);
      console.log(`   Create it manually at: https://github.com/<owner>/<repo>/compare/master...${releaseBranch}`);
    }
    
    // Create GitHub release
    await createGitHubRelease(releaseNotes);
    
    // Switch back to original branch
    run("git checkout master");
  }

  console.log(`\n🎉 Release ${dryRun ? "validation" : "process"} complete!`);

  if (dryRun) {
    console.log(
      "\n📋 Dry run complete. To perform the actual release, run without --dry flag:"
    );
    console.log(`   node release.js ${version}`);
  } else {
    console.log("\n📋 Release completed successfully!");
    console.log("   - Version tags created and pushed");
    console.log("   - Pull request created for version changes");
    console.log("   - GitHub release created");
    console.log("   - All crates published to crates.io");
    console.log(`   - Release notes saved in releases/${version}-RELEASE.md`);
    console.log("\n⚠️  Next step: Review and merge the pull request to complete the release");
  }
}

// Run the release process
release().catch((error) => {
  console.error("❌ Release failed:", error);
  process.exit(1);
});
