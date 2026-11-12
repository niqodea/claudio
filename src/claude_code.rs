//! The Claude Code that claudio runs: one version, the one claudio's assumptions on Claude Code
//! were checked against, in claudio's own data directory. A `claude` the user runs apart from
//! claudio keeps its own version, so updating it never changes how claudio works.
//!
//! claudio only runs its copy, and `claudio install-claude-code` installs it, as
//! `playwright install` installs the browser builds a Playwright release runs.
//! Ref: <https://playwright.dev/docs/browsers>, on Playwright's own copies of browsers

use std::env;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

/// The version claudio runs and was checked against: each check of [`crate::contract`] passes
/// with it.
pub const VERSION: &str = "2.1.322";

/// The sha256 of the binary of each platform, from the manifest of the release, next to its
/// binaries.
/// Ref: <https://downloads.claude.ai/claude-code-releases/2.1.322/manifest.json>
const CHECKSUMS: [(&str, &str); 4] = [
    (
        "darwin-arm64",
        "a9d237949db816a0b7afd88a9a8e1df36b3a597a30f5a41d1c0539eb180a3450",
    ),
    (
        "darwin-x64",
        "b513d15117cbf1bb05095dab41963fd394ff40de1fa99dcc6d7d1033bf292e9b",
    ),
    (
        "linux-arm64",
        "f220ca50ea4f3222043ff83cc224a1262e0196e9897acc0871efa94235b0c1fd",
    ),
    (
        "linux-x64",
        "bfb3703ad5380242328459bc62ada4bde8ae7ba57c6e1eeadb9c00e08874c478",
    ),
];

/// The subcommand that installs the copy.
pub const INSTALL_COMMAND: &str = "install-claude-code";

/// The variable that names another Claude Code to run instead, like a newer one to try before
/// claudio is checked against it, which a warning then names.
const OVERRIDE: &str = "CLAUDIO_CLAUDE";

/// The Claude Code to run: the one the override names, or else claudio's copy.
pub fn program() -> Result<PathBuf, String> {
    if let Some(program) = env::var_os(OVERRIDE).filter(|program| !program.is_empty()) {
        return Ok(PathBuf::from(program));
    }
    let path = copy_path()?;
    if !path.exists() {
        return Err(format!(
            "Claude Code {VERSION} is missing: claudio {INSTALL_COMMAND} installs it"
        ));
    }
    Ok(path)
}

/// The command that runs `program` as claudio talks to it, in stream-json both ways.
pub fn command(program: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .args([
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            // stream-json output needs it
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
        ])
        // The copy keeps the version claudio was checked against
        // Ref: <https://code.claude.com/docs/en/settings#environment-variables>
        .env("DISABLE_AUTOUPDATER", "1");
    send_session_state(&mut command);
    command
}

/// Makes Claude Code send its session state, which tells when Claude works.
///
/// Undocumented: [`crate::contract::session_state_comes_with_its_variable`]
fn send_session_state(command: &mut Command) {
    command.env("CLAUDE_CODE_SDK_READS_SESSION_STATE", "1");
}

/// Downloads claudio's copy and checks it against its sha256, then removes the copies of other
/// versions, which claudio no longer runs.
pub fn install() -> Result<(), String> {
    let path = copy_path()?;
    let directory = path.parent().expect("the copy is in a directory");
    if path.exists() {
        println!(
            "Claude Code {VERSION} is already installed in {}",
            directory.display()
        );
    } else {
        let (platform, checksum) = platform()?;
        println!("Downloading Claude Code {VERSION} for {platform}…");
        fs::create_dir_all(directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        // A download that fails leaves the partial file, which the next one overwrites
        let partial = directory.join("claude.part");
        download(platform, &partial)?;
        check(&partial, checksum)?;
        fs::set_permissions(&partial, fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("cannot make {} executable: {error}", partial.display()))?;
        fs::rename(&partial, &path)
            .map_err(|error| format!("cannot move {}: {error}", partial.display()))?;
        println!(
            "Installed Claude Code {VERSION} for {platform} in {}",
            directory.display()
        );
    }
    remove_other_versions(directory.parent().expect("the version is in a directory"))
}

/// The path of claudio's copy: `claudio/claude-code/<version>/claude` in `$XDG_DATA_HOME`, or in
/// `~/.local/share` when that is unset or empty.
fn copy_path() -> Result<PathBuf, String> {
    let data = match env::var_os("XDG_DATA_HOME") {
        Some(directory) if !directory.is_empty() => PathBuf::from(directory),
        _ => PathBuf::from(env::var_os("HOME").ok_or("HOME is unset")?).join(".local/share"),
    };
    Ok(data
        .join("claudio/claude-code")
        .join(VERSION)
        .join("claude"))
}

/// The platform's name in the release, with the sha256 of its binary.
fn platform() -> Result<(&'static str, &'static str), String> {
    let name = match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("linux", "x86_64") => "linux-x64",
        (os, arch) => return Err(format!("Claude Code has no release for {os} on {arch}")),
    };
    let (_, checksum) = CHECKSUMS
        .iter()
        .find(|(platform, _)| *platform == name)
        .expect("each platform has its checksum");
    Ok((name, checksum))
}

/// Downloads the binary with curl, which shows its progress.
fn download(platform: &str, to: &Path) -> Result<(), String> {
    let url =
        format!("https://downloads.claude.ai/claude-code-releases/{VERSION}/{platform}/claude");
    let status = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--progress-bar",
            "--max-time",
            "600",
            "--output",
        ])
        .arg(to)
        .arg(&url)
        .status()
        .map_err(|error| format!("cannot run curl: {error}"))?;
    if !status.success() {
        return Err(format!("cannot download {url}: curl ended with {status}"));
    }
    Ok(())
}

fn check(path: &Path, expected: &str) -> Result<(), String> {
    let mut file =
        fs::File::open(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        return Err(format!(
            "{} has sha256 {actual}, not the pinned {expected}",
            path.display()
        ));
    }
    Ok(())
}

/// Removes the copies of versions other than the one claudio runs, from `versions`, the
/// directory of all copies.
fn remove_other_versions(versions: &Path) -> Result<(), String> {
    let entries = match fs::read_dir(versions) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot read {}: {error}", versions.display())),
    };
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("cannot read {}: {error}", versions.display()))?;
        if entry.file_name() == VERSION {
            continue;
        }
        fs::remove_dir_all(entry.path())
            .map_err(|error| format!("cannot remove {}: {error}", entry.path().display()))?;
        println!(
            "Removed Claude Code {}",
            entry.file_name().to_string_lossy()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_passes_only_its_own_sha256() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("claude");
        fs::write(&path, "abc").expect("a written file");
        // The sha256 of "abc", from FIPS 180-2's examples
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(check(&path, abc).is_ok());
        assert!(check(&path, &"0".repeat(64)).is_err());
    }

    #[test]
    fn only_the_copy_of_the_version_claudio_runs_stays() {
        let versions = tempfile::tempdir().expect("a temporary directory");
        for version in [VERSION, "2.1.300", "2.1.301"] {
            fs::create_dir(versions.path().join(version)).expect("a version's directory");
        }
        remove_other_versions(versions.path()).expect("the other versions removed");
        let left: Vec<_> = fs::read_dir(versions.path())
            .expect("the directory")
            .map(|entry| entry.expect("an entry").file_name())
            .collect();
        assert_eq!(left, [VERSION]);
    }
}
