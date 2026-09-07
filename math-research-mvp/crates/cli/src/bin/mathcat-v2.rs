//! Standalone 2.5.0 process. Defaults and all writable paths stay in the new copy.
use std::{
    io::Write,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::Parser;
use research_core::research_v2::{V2Config, V2Service};
use research_domain::research_v2::VERSION;
use research_storage::research_v2::V2Store;

#[derive(Debug, Parser)]
#[command(name="mathcat-v2", version=VERSION, about="MathCat Lab independent research 2.5.0 server")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:8899")]
    bind: SocketAddr,
    #[arg(long)]
    database: Option<PathBuf>,
    #[arg(long)]
    data_root: Option<PathBuf>,
    #[arg(long, default_value = if cfg!(windows) { "codex.cmd" } else { "codex" })]
    codex_command: PathBuf,
    #[arg(long)]
    token_file: Option<PathBuf>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long, value_parser = ["minimal", "low", "medium", "high", "xhigh"])]
    reasoning_effort: Option<String>,
    #[arg(long, default_value_t = 1800)]
    turn_timeout_seconds: u64,
}

fn version_root() -> Result<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .context("Cannot locate the version-isolated root")?;
    root.canonicalize().context("Version root must exist")
}

// canonicalize() adds Windows' extended-length prefix; launchers use the equivalent
// ordinary absolute path. Normalize ONLY that spelling for the lexical boundary check.
// The existing ancestor is still canonicalized separately to reject junction escapes.
fn comparable_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::{ffi::OsString, path::Prefix};
        let mut result = PathBuf::new();
        for component in path.components() {
            match component {
                Component::Prefix(prefix) => match prefix.kind() {
                    Prefix::VerbatimDisk(drive) => result.push(format!("{}:", char::from(drive))),
                    Prefix::VerbatimUNC(server, share) => {
                        let mut unc = OsString::from("\\\\");
                        unc.push(server);
                        unc.push("\\");
                        unc.push(share);
                        result.push(unc);
                    }
                    _ => result.push(component.as_os_str()),
                },
                _ => result.push(component.as_os_str()),
            }
        }
        result
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

fn checked_path(root: &Path, requested: Option<PathBuf>, fallback: &str) -> Result<PathBuf> {
    let requested = requested.unwrap_or_else(|| root.join(fallback));
    let absolute = if requested.is_absolute() {
        requested
    } else {
        root.join(requested)
    };
    if absolute
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        bail!("Parent traversal is not accepted for writable v2 paths");
    }
    let canonical_root = root.canonicalize().context("Version root must exist")?;
    if !comparable_path(&absolute).starts_with(comparable_path(&canonical_root)) {
        bail!("Writable paths must remain inside the new MathCat 2.5.0 copy");
    }
    let existing = absolute
        .ancestors()
        .find(|path| std::fs::symlink_metadata(path).is_ok())
        .context("No existing path ancestor")?
        .canonicalize()?;
    if !existing.starts_with(&canonical_root) {
        bail!("A writable path resolves outside the new version through a link");
    }
    Ok(absolute)
}

fn api_token(path: &Path) -> Result<String> {
    if path.exists() {
        if !path.is_file() {
            bail!("API token path is not a regular file");
        }
        let token = std::fs::read_to_string(path)?.trim().to_owned();
        if token.len() < 32
            || token.len() > 512
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        {
            bail!("Existing API token file has an invalid format; its content was not printed");
        }
        return Ok(token);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Two ULIDs supply two independent 80-bit random fields. No token is logged.
    let token = format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new());
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return api_token(path),
        Err(error) => return Err(error.into()),
    };
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    Ok(token)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if !args.bind.ip().is_loopback() {
        bail!("MathCat v2 only binds a loopback address; remote exposure is not enabled");
    }
    if args.turn_timeout_seconds == 0 {
        bail!("turn-timeout-seconds must be positive");
    }
    let root = version_root()?;
    let database = checked_path(&root, args.database, "runtime/research-v2/state_v2.sqlite")?;
    let data_root = checked_path(&root, args.data_root, "workspaces")?;
    let token_file = checked_path(&root, args.token_file, "runtime/api-token")?;
    // Acquire the port before opening writable stores or recovering any process.
    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .context("The v2 port is unavailable; no legacy service was stopped")?;
    let token = api_token(&token_file)?;
    let store = V2Store::connect(&database, &data_root).await?;
    let config = V2Config {
        codex_command: args.codex_command,
        model: args.model,
        reasoning_effort: args.reasoning_effort,
        turn_timeout_seconds: args.turn_timeout_seconds,
    };
    let service = V2Service::new(store, config);
    service.recover().await?;
    let app = research_api::research_v2::router(service.clone(), token);
    eprintln!(
        "MathCat Lab {VERSION} is listening on http://{} (v2 only; token stays server-side)",
        args.bind
    );
    let stop_service = service.clone();
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = stop_service.shutdown().await;
        })
        .await;
    let _ = service.shutdown().await;
    result.context("MathCat v2 HTTP server failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writable_paths_reject_legacy_and_parent_escape() {
        let root = version_root().unwrap();
        assert!(checked_path(&root, Some(root.join("../legacy.db")), "unused").is_err());
        assert!(checked_path(&root, root.parent().map(|p| p.join("old.db")), "unused").is_err());
        assert!(
            checked_path(&root, None, "runtime/test.db")
                .unwrap()
                .starts_with(root)
        );
    }

    #[test]
    fn default_is_distinct_loopback_port() {
        let args = Args::parse_from(["mathcat-v2"]);
        assert!(args.bind.ip().is_loopback());
        assert_eq!(args.bind.port(), 8899);
        assert!(args.database.is_none());
        assert!(args.token_file.is_none());
    }

    #[test]
    fn explicit_ordinary_absolute_paths_inside_new_version_are_allowed() {
        let root = version_root().unwrap();
        let ordinary = comparable_path(&root);
        #[cfg(windows)]
        assert!(!ordinary.to_string_lossy().starts_with("\\\\?\\"));
        for suffix in [
            "runtime/research-v2/state_v2.sqlite",
            "runtime/api-token",
            "workspaces",
        ] {
            let requested = ordinary.join(suffix);
            assert!(
                checked_path(&root, Some(requested.clone()), "unused").is_ok(),
                "explicit path rejected: {requested:?}"
            );
        }
        assert!(
            checked_path(
                &root,
                ordinary
                    .parent()
                    .map(|p| p.join("MathCat-Lab-1.0.0/runtime/state.sqlite")),
                "unused"
            )
            .is_err()
        );
        assert!(
            checked_path(
                &root,
                Some(ordinary.join("runtime/../state.sqlite")),
                "unused"
            )
            .is_err()
        );
    }

    #[test]
    fn linked_ancestor_cannot_escape_version_root() {
        let temporary =
            std::env::temp_dir().join(format!("mathcat-v2-path-test-{}", ulid::Ulid::new()));
        std::fs::create_dir(&temporary).unwrap();
        let root = temporary.join("new-version");
        let outside = temporary.join("old-version");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let linked = root.join("linked");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &linked).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Directory junctions need no developer mode or symlink privilege. All targets
            // are generated inside one unique test directory; cleanup below is non-recursive.
            let status = std::process::Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:MATHCAT_TEST_LINK -Target $env:MATHCAT_TEST_TARGET | Out-Null",
                ])
                .env("MATHCAT_TEST_LINK",&linked)
                .env("MATHCAT_TEST_TARGET",&outside)
                .creation_flags(0x0800_0000)
                .status()
                .unwrap();
            assert!(status.success());
        }
        assert!(
            checked_path(
                &root.canonicalize().unwrap(),
                Some(linked.join("must-not-write.sqlite")),
                "unused"
            )
            .is_err()
        );
        std::fs::remove_dir(&outside).unwrap();
        assert!(
            checked_path(
                &root.canonicalize().unwrap(),
                Some(linked.join("dangling.sqlite")),
                "unused"
            )
            .is_err()
        );
        #[cfg(windows)]
        std::fs::remove_dir(&linked).unwrap();
        #[cfg(unix)]
        std::fs::remove_file(&linked).unwrap();
        std::fs::remove_dir(root).unwrap();
        std::fs::remove_dir(temporary).unwrap();
    }
}
