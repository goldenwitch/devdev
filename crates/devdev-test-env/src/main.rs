//! `devdev-test-env` binary entry-point.
//!
//! Five subcommands, all driven from a single committed manifest +
//! lock file pair under `test-env/`. Admin tokens come from the
//! environment; we never write them anywhere.
//!
//! ```text
//! devdev-test-env apply
//! devdev-test-env verify
//! devdev-test-env reset-comments --admin-github-login=...
//!                                --admin-ado-name=...
//! devdev-test-env destroy --yes-really
//! devdev-test-env print-env
//! ```

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use devdev_test_env::manifest::{Manifest, ManifestLock};
use devdev_test_env::{ado, github, reset};

#[derive(Parser, Debug, Clone)]
#[command(name = "devdev-test-env", about = "Provision DevDev live-test fixtures")]
struct Cli {
    /// Manifest path. Defaults to `test-env/manifest.json` relative to cwd.
    #[arg(long, global = true, default_value = "test-env/manifest.json")]
    manifest: PathBuf,
    /// Lock file path. Defaults to `test-env/manifest.lock.json`.
    #[arg(long, global = true, default_value = "test-env/manifest.lock.json")]
    lock: PathBuf,
    /// Skip the GitHub side (only act on ADO).
    #[arg(long, global = true)]
    skip_github: bool,
    /// Skip the ADO side (only act on GitHub).
    #[arg(long, global = true)]
    skip_ado: bool,
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand, Debug, Clone)]
enum Command {
    /// Provision (or reconcile) fixtures to match the manifest.
    Apply,
    /// Read-only check that fixtures match the manifest.
    Verify,
    /// Sweep stray comments off the canonical PRs.
    ResetComments {
        /// GitHub login of the admin identity (used to decide which
        /// comments are admin-pinned vs test-issued).
        #[arg(long, env = "DEVDEV_TEST_ENV_GITHUB_ADMIN_LOGIN")]
        admin_github_login: String,
        /// ADO `uniqueName` (usually an email) of the admin identity.
        #[arg(long, env = "DEVDEV_TEST_ENV_ADO_ADMIN_NAME")]
        admin_ado_name: String,
    },
    /// Tear down fixtures. Disabled unless `--yes-really` is set.
    Destroy {
        #[arg(long)]
        yes_really: bool,
    },
    /// Emit the env-var block downstream test runners should consume.
    PrintEnv,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    match cli.cmd.clone() {
        Command::Apply => apply(&cli).await,
        Command::Verify => verify(&cli).await,
        Command::ResetComments {
            admin_github_login,
            admin_ado_name,
        } => reset_comments(&cli, &admin_github_login, &admin_ado_name).await,
        Command::Destroy { yes_really } => destroy(&cli, yes_really).await,
        Command::PrintEnv => print_env(&cli).await,
    }
}

fn read_manifest(path: &Path) -> anyhow::Result<Manifest> {
    Manifest::read(path)
}

fn read_lock(path: &Path) -> anyhow::Result<ManifestLock> {
    ManifestLock::read_or_default(path)
}

async fn apply(cli: &Cli) -> anyhow::Result<()> {
    let manifest = read_manifest(&cli.manifest)?;
    let mut lock = read_lock(&cli.lock)?;

    if !cli.skip_github {
        let token = std::env::var("GITHUB_TOKEN_ADMIN").map_err(|_| {
            anyhow::anyhow!("GITHUB_TOKEN_ADMIN must be set for `apply --skip-github=false`")
        })?;
        let client = github::GithubClient::new(token)?;
        let resolved = client.apply(&manifest.github).await?;
        tracing::info!(
            org = %manifest.github.org,
            repo = %manifest.github.repo,
            pr = resolved.canonical_pr_number,
            "github fixture applied",
        );
        lock.github = Some(resolved);
    }

    if !cli.skip_ado {
        let pat = std::env::var("ADO_PAT_ADMIN").map_err(|_| {
            anyhow::anyhow!("ADO_PAT_ADMIN must be set for `apply --skip-ado=false`")
        })?;
        let client = ado::AdoClient::new(&pat)?;
        let resolved = client.apply(&manifest.azure_devops).await?;
        tracing::info!(
            org = %manifest.azure_devops.org,
            project = %manifest.azure_devops.project,
            repo = %manifest.azure_devops.repo,
            pr = resolved.canonical_pr_id,
            "ado fixture applied",
        );
        lock.azure_devops = Some(resolved);
    }

    lock.write(&cli.lock)?;
    println!("OK: lock written to {}", cli.lock.display());
    Ok(())
}

async fn verify(cli: &Cli) -> anyhow::Result<()> {
    let manifest = read_manifest(&cli.manifest)?;
    let lock = read_lock(&cli.lock)?;

    if !cli.skip_github {
        let lock_gh = lock
            .github
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no github lock entry; run `apply` first"))?;
        let token = std::env::var("GITHUB_TOKEN_ADMIN")
            .or_else(|_| std::env::var("GITHUB_TOKEN"))
            .map_err(|_| {
                anyhow::anyhow!("GITHUB_TOKEN_ADMIN or GITHUB_TOKEN must be set for verify")
            })?;
        let client = github::GithubClient::new(token)?;
        client.verify(&manifest.github, lock_gh).await?;
        println!("OK: github fixture matches manifest");
    }

    if !cli.skip_ado {
        let lock_ado = lock
            .azure_devops
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no ado lock entry; run `apply` first"))?;
        let pat = std::env::var("ADO_PAT_ADMIN")
            .or_else(|_| std::env::var("ADO_PAT"))
            .map_err(|_| anyhow::anyhow!("ADO_PAT_ADMIN or ADO_PAT must be set for verify"))?;
        let client = ado::AdoClient::new(&pat)?;
        client.verify(&manifest.azure_devops, lock_ado).await?;
        println!("OK: ado fixture matches manifest");
    }

    Ok(())
}

async fn reset_comments(
    cli: &Cli,
    admin_gh: &str,
    admin_ado: &str,
) -> anyhow::Result<()> {
    let manifest = read_manifest(&cli.manifest)?;
    let lock = read_lock(&cli.lock)?;

    if !cli.skip_github {
        let lock_gh = lock
            .github
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no github lock entry; run `apply` first"))?;
        let token = std::env::var("GITHUB_TOKEN_ADMIN")
            .map_err(|_| anyhow::anyhow!("GITHUB_TOKEN_ADMIN required for reset-comments"))?;
        let client = github::GithubClient::new(token)?;
        let n = reset::reset_github_comments(&client, &manifest.github, lock_gh, admin_gh).await?;
        println!("OK: github reset-comments deleted {n}");
    }

    if !cli.skip_ado {
        let lock_ado = lock
            .azure_devops
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no ado lock entry; run `apply` first"))?;
        let pat = std::env::var("ADO_PAT_ADMIN")
            .map_err(|_| anyhow::anyhow!("ADO_PAT_ADMIN required for reset-comments"))?;
        let client = ado::AdoClient::new(&pat)?;
        let n =
            reset::reset_ado_comments(&client, &manifest.azure_devops, lock_ado, admin_ado).await?;
        println!("OK: ado reset-comments deleted {n}");
    }

    Ok(())
}

async fn destroy(_cli: &Cli, yes_really: bool) -> anyhow::Result<()> {
    if !yes_really {
        anyhow::bail!("destroy requires `--yes-really`; aborting");
    }
    // First-cut: not implemented. Manual teardown via the host UI is
    // safer for now (we don't want a misconfigured cron eating the
    // fixture org). Surface this clearly rather than silently
    // pretending to succeed.
    anyhow::bail!(
        "destroy is not implemented in the first cut; tear down fixtures manually \
         (see docs/internals/live-test-fixtures.md)"
    )
}

async fn print_env(cli: &Cli) -> anyhow::Result<()> {
    let manifest = read_manifest(&cli.manifest)?;
    let lock = read_lock(&cli.lock)?;

    if let Some(gh) = &lock.github {
        println!("DEVDEV_LIVE_GITHUB=1");
        println!(
            "DEVDEV_GH_PR_URL=https://github.com/{}/{}/pull/{}",
            manifest.github.org, manifest.github.repo, gh.canonical_pr_number
        );
        println!("DEVDEV_GH_FIXTURE_ORG={}", manifest.github.org);
        println!("DEVDEV_GH_FIXTURE_REPO={}", manifest.github.repo);
    }
    if let Some(ado) = &lock.azure_devops {
        println!("DEVDEV_LIVE_ADO=1");
        println!(
            "DEVDEV_ADO_PR_URL=https://dev.azure.com/{}/{}/_git/{}/pullrequest/{}",
            manifest.azure_devops.org,
            manifest.azure_devops.project,
            manifest.azure_devops.repo,
            ado.canonical_pr_id,
        );
        println!("DEVDEV_ADO_FIXTURE_ORG={}", manifest.azure_devops.org);
        println!("DEVDEV_ADO_FIXTURE_PROJECT={}", manifest.azure_devops.project);
        println!("DEVDEV_ADO_FIXTURE_REPO={}", manifest.azure_devops.repo);
    }
    Ok(())
}
