//! Manifest types and on-disk format.
//!
//! `manifest.json` is the *declarative* source of truth: org/project
//! names, fixture branch + PR shape, the canonical commit message.
//! `manifest.lock.json` is the *materialised* state: PR numbers and
//! other server-assigned ids the provisioner backfills after first
//! apply. Lock-file pattern matches `Cargo.lock` semantics — committed,
//! regenerable, but stable across runs once written.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Top-level manifest committed to `test-env/manifest.json`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub github: GithubFixture,
    pub azure_devops: AdoFixture,
}

/// GitHub fixture description.
///
/// The org is *asserted* (manual provisioning), every other resource
/// is ensured by `apply`. `comment_tag_prefix` is the literal string
/// every test-issued comment must start with so `reset-comments` can
/// distinguish them from the canonical PR description / admin-pinned
/// notes that must survive cleanup.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GithubFixture {
    pub org: String,
    pub repo: String,
    pub default_branch: String,
    pub fixture_branch: String,
    pub canonical_pr: CanonicalPr,
    pub comment_tag_prefix: String,
}

/// Azure DevOps fixture description.
///
/// ADO addresses repos as `{org}/{project}/_git/{repo}`. Our adapter
/// encodes this as `owner = "{org}/{project}"`, `repo = "{repo}"`;
/// the manifest keeps them split because that's how ADO's REST API
/// wants them too.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdoFixture {
    pub org: String,
    pub project: String,
    pub repo: String,
    pub default_branch: String,
    pub fixture_branch: String,
    pub canonical_pr: CanonicalPr,
    pub comment_tag_prefix: String,
}

/// Canonical PR shape — the same struct on both hosts. The PR
/// number itself lives in [`ManifestLock`], not here.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CanonicalPr {
    /// Title — used as the PR's title and as part of the
    /// `reset-comments` allow-list match.
    pub title: String,
    /// Body — admin-authored description. Tests must NOT modify
    /// this. `verify` asserts it byte-for-byte.
    pub body: String,
    /// Branch the PR targets (defaults to `default_branch` if not
    /// specified, but explicit avoids the merge-conflict footgun).
    pub base: String,
    /// Files committed on `fixture_branch` to seed the diff. Empty
    /// vec is allowed — produces a no-change PR (still valid).
    #[serde(default)]
    pub fixture_files: Vec<FixtureFile>,
}

/// Single file checked into the fixture branch. Contents are
/// rewritten on every `apply` if they drift, so this is the only
/// place to edit fixture content.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FixtureFile {
    pub path: String,
    pub contents: String,
}

/// Lock file written to `test-env/manifest.lock.json` after first
/// `apply`. Carries server-assigned ids that didn't exist at
/// manifest-edit time. Committed alongside the manifest.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManifestLock {
    /// Per-host resolved state. `None` means the corresponding
    /// `apply` hasn't been run yet (fresh checkout).
    #[serde(default)]
    pub github: Option<GithubLock>,
    #[serde(default)]
    pub azure_devops: Option<AdoLock>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GithubLock {
    pub repo_id: u64,
    pub canonical_pr_number: u64,
    pub canonical_pr_node_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdoLock {
    pub repo_id: String,
    pub canonical_pr_id: u64,
}

impl Manifest {
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| anyhow::anyhow!("failed to read manifest at {}: {e}", path.display()))?;
        let manifest: Manifest = serde_json::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!("failed to parse manifest at {}: {e}", path.display()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Cheap structural checks. Things the REST API would reject
    /// later anyway, surfaced earlier with a useful diagnostic.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.github.org.is_empty() || self.github.repo.is_empty() {
            anyhow::bail!("github.org and github.repo must be non-empty");
        }
        if self.azure_devops.org.is_empty()
            || self.azure_devops.project.is_empty()
            || self.azure_devops.repo.is_empty()
        {
            anyhow::bail!("azure_devops.{{org,project,repo}} must all be non-empty");
        }
        if self.github.canonical_pr.base != self.github.default_branch {
            anyhow::bail!(
                "github canonical_pr.base ({}) must currently equal default_branch ({}); \
                 cross-branch fixtures aren't supported in the first cut",
                self.github.canonical_pr.base,
                self.github.default_branch,
            );
        }
        if !self.github.comment_tag_prefix.starts_with('[')
            || !self.github.comment_tag_prefix.ends_with(']')
        {
            anyhow::bail!(
                "github.comment_tag_prefix must be of the form `[devdev-live-test...]`"
            );
        }
        if !self.azure_devops.comment_tag_prefix.starts_with('[')
            || !self.azure_devops.comment_tag_prefix.ends_with(']')
        {
            anyhow::bail!("azure_devops.comment_tag_prefix must be `[...]`-bracketed");
        }
        Ok(())
    }
}

impl ManifestLock {
    pub fn read_or_default(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(ManifestLock::default());
        }
        let bytes = std::fs::read(path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn write(&self, path: &Path) -> anyhow::Result<()> {
        let pretty = serde_json::to_string_pretty(self)?;
        std::fs::write(path, format!("{pretty}\n"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        Manifest {
            github: GithubFixture {
                org: "devdev-fixtures".into(),
                repo: "live-tests".into(),
                default_branch: "main".into(),
                fixture_branch: "fixture/canonical".into(),
                canonical_pr: CanonicalPr {
                    title: "Canonical fixture PR — DO NOT MERGE".into(),
                    body: "This PR is provisioned by devdev-test-env.".into(),
                    base: "main".into(),
                    fixture_files: vec![FixtureFile {
                        path: "FIXTURE.md".into(),
                        contents: "fixture\n".into(),
                    }],
                },
                comment_tag_prefix: "[devdev-live-test]".into(),
            },
            azure_devops: AdoFixture {
                org: "devdev-fixtures".into(),
                project: "DevDev-Live".into(),
                repo: "live-tests".into(),
                default_branch: "main".into(),
                fixture_branch: "fixture/canonical".into(),
                canonical_pr: CanonicalPr {
                    title: "Canonical fixture PR — DO NOT MERGE".into(),
                    body: "Provisioned by devdev-test-env.".into(),
                    base: "main".into(),
                    fixture_files: vec![],
                },
                comment_tag_prefix: "[devdev-live-test]".into(),
            },
        }
    }

    #[test]
    fn validate_accepts_sample() {
        sample().validate().unwrap();
    }

    #[test]
    fn validate_rejects_empty_org() {
        let mut m = sample();
        m.github.org = String::new();
        assert!(m.validate().is_err());
    }

    #[test]
    fn validate_rejects_unbracketed_tag_prefix() {
        let mut m = sample();
        m.github.comment_tag_prefix = "devdev-live-test".into();
        let err = m.validate().unwrap_err().to_string();
        assert!(err.contains("comment_tag_prefix"), "diagnostic was: {err}");
    }

    #[test]
    fn validate_rejects_cross_branch_pr() {
        let mut m = sample();
        m.github.canonical_pr.base = "develop".into();
        let err = m.validate().unwrap_err().to_string();
        assert!(err.contains("default_branch"), "diagnostic was: {err}");
    }

    #[test]
    fn manifest_round_trips_through_json() {
        let json = serde_json::to_string_pretty(&sample()).unwrap();
        let back: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(sample(), back);
    }

    #[test]
    fn lock_default_has_no_resolved_state() {
        let lock = ManifestLock::default();
        assert!(lock.github.is_none());
        assert!(lock.azure_devops.is_none());
    }

    #[test]
    fn lock_round_trips_through_json() {
        let lock = ManifestLock {
            github: Some(GithubLock {
                repo_id: 12345,
                canonical_pr_number: 7,
                canonical_pr_node_id: "PR_kwDO".into(),
            }),
            azure_devops: Some(AdoLock {
                repo_id: "abcd-ef".into(),
                canonical_pr_id: 42,
            }),
        };
        let json = serde_json::to_string_pretty(&lock).unwrap();
        let back: ManifestLock = serde_json::from_str(&json).unwrap();
        assert_eq!(lock, back);
    }

    #[test]
    fn lock_rejects_unknown_fields() {
        let bad = r#"{"github": null, "azure_devops": null, "stray": 1}"#;
        let err = serde_json::from_str::<ManifestLock>(bad).unwrap_err();
        assert!(err.to_string().contains("stray"), "got: {err}");
    }

    #[test]
    fn read_returns_diagnostic_for_missing_file() {
        let err = Manifest::read(Path::new("does/not/exist.json")).unwrap_err();
        assert!(
            err.to_string().contains("failed to read manifest"),
            "diagnostic was: {err}"
        );
    }
}
