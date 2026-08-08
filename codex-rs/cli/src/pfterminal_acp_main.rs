//! `pfterminal-acp` — the stable ACP entry point for PFTerminal.
//!
//! ACP (Agent Client Protocol) clients such as Buzz speak JSON-RPC over stdio.
//! PFTerminal does not speak ACP directly; it exposes the Codex app-server
//! protocol. The maintained `codex-acp` adapter translates between the two and
//! honours `CODEX_PATH` to select which Codex-compatible binary it drives.
//!
//! So this binary is deliberately *not* an ACP implementation. It is a thin,
//! stable launcher that:
//!
//!   1. resolves the PFTerminal executable,
//!   2. sets `CODEX_PATH` authoritatively,
//!   3. hands off to `codex-acp`,
//!   4. preserves stdin/stdout/stderr, signals, and the adapter's exit code,
//!   5. writes every *runtime* diagnostic to stderr.
//!
//! Point 5 is not a style preference. Once the handoff happens, stdout carries
//! ACP protocol frames exclusively — a single stray line on stdout corrupts the
//! stream and the client will fail to parse it. The sole exception is the
//! `--version` / `--help` output below, which goes to stdout because that is
//! what every other CLI does and because redirecting `--version` to stderr
//! breaks ordinary shell usage. Those flags are only ever passed by a human or
//! an install probe; ACP never sends them, and such an invocation exits before
//! any protocol traffic exists.
//!
//! Point 4 is why the Unix path uses `exec()` rather than spawn-and-wait: the
//! adapter *becomes* this process, so Ctrl-C, SIGTERM and exit codes need no
//! forwarding logic that could get them subtly wrong. Windows has no `exec`, so
//! it spawns and then exits with the adapter's raw code via `process::exit` —
//! not `ExitCode`, which is a `u8` and would fold 256 to 0 and 3010 to 194.
//!
//! Why a distinct command rather than telling users to run
//! `CODEX_PATH=pfterminal codex-acp`: ACP hosts key runtime identity off the
//! command. Buzz's built-in Codex runtime is already defined around `codex-acp`,
//! so sharing that executable risks mistaken runtime identity, stale persona
//! pins, and merged usage attribution between two different agents.

use std::env;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

/// Environment variable that pins the PFTerminal executable explicitly.
const PFTERMINAL_PATH_ENV: &str = "PFTERMINAL_PATH";
/// Environment variable that pins the adapter executable explicitly.
const CODEX_ACP_PATH_ENV: &str = "CODEX_ACP_PATH";
/// The variable `codex-acp` reads to choose which binary to drive.
const CODEX_PATH_ENV: &str = "CODEX_PATH";

const ADAPTER_BIN: &str = "codex-acp";
const PFTERMINAL_BIN: &str = "pfterminal";

const ADAPTER_INSTALL_HINT: &str = "install it with `npm install -g @agentclientprotocol/codex-acp`, or set \
     CODEX_ACP_PATH to its executable";

fn main() -> std::process::ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();

    // Handled locally rather than forwarded: `pfterminal-acp --version` should
    // describe *this* launcher and what it resolved, which is the only thing
    // that makes it useful for diagnosing a broken install. ACP itself never
    // sends these flags, so intercepting them cannot affect a live session.
    if let Some(first) = args.first() {
        if first == "--version" || first == "-V" {
            print_version();
            return std::process::ExitCode::SUCCESS;
        }
        if first == "--help" || first == "-h" {
            print_help();
            return std::process::ExitCode::SUCCESS;
        }
    }

    let pfterminal = match resolve_pfterminal() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("pfterminal-acp: {err}");
            return std::process::ExitCode::from(127);
        }
    };

    let adapter = match resolve_adapter() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("pfterminal-acp: {err}");
            return std::process::ExitCode::from(127);
        }
    };

    let mut command = Command::new(&adapter);
    command.args(&args);
    // Authoritative: a CODEX_PATH inherited from the environment would silently
    // point the adapter at a different agent while the client still believes it
    // is talking to PFTerminal.
    command.env(CODEX_PATH_ENV, &pfterminal);

    exec(command, &adapter)
}

/// Replace this process with the adapter, preserving stdio, signals and status.
#[cfg(unix)]
fn exec(mut command: Command, adapter: &Path) -> ! {
    use std::os::unix::process::CommandExt;
    // `exec` only returns on failure.
    let err = command.exec();
    eprintln!(
        "pfterminal-acp: failed to execute {}: {err}",
        adapter.display()
    );
    std::process::exit(126)
}

/// Windows has no `exec`; spawn and exit with the adapter's own code.
///
/// `process::exit` takes an `i32`, so the adapter's status survives intact.
/// Returning `ExitCode` here would silently truncate to a `u8`: 256 would
/// arrive as success, 3010 (the common "reboot required" code) as 194, and
/// NTSTATUS-style negatives as arbitrary low bytes.
#[cfg(not(unix))]
fn exec(mut command: Command, adapter: &Path) -> ! {
    match command.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(err) => {
            eprintln!(
                "pfterminal-acp: failed to execute {}: {err}",
                adapter.display()
            );
            std::process::exit(126)
        }
    }
}

/// Locate the PFTerminal executable.
///
/// Sibling-of-self is tried before `PATH` because the two binaries ship
/// together: when a user has several PFTerminal installs, the adapter should
/// drive the one it was installed alongside, not whichever happens to win on
/// `PATH`.
fn resolve_pfterminal() -> Result<PathBuf, String> {
    if let Some(explicit) = env::var_os(PFTERMINAL_PATH_ENV) {
        let path = PathBuf::from(explicit);
        if is_executable(&path) {
            return Ok(absolute(path));
        }
        return Err(format!(
            "{PFTERMINAL_PATH_ENV} is set to {} but that is not an executable file",
            path.display()
        ));
    }

    if let Some(sibling) = sibling_executable(PFTERMINAL_BIN) {
        return Ok(absolute(sibling));
    }

    if let Some(found) = find_on_path(PFTERMINAL_BIN) {
        return Ok(absolute(found));
    }

    Err(format!(
        "could not find the `{PFTERMINAL_BIN}` executable next to this binary or on PATH; \
         set {PFTERMINAL_PATH_ENV} to its location"
    ))
}

fn resolve_adapter() -> Result<PathBuf, String> {
    if let Some(explicit) = env::var_os(CODEX_ACP_PATH_ENV) {
        let path = PathBuf::from(explicit);
        if is_executable(&path) {
            return Ok(path);
        }
        return Err(format!(
            "{CODEX_ACP_PATH_ENV} is set to {} but that is not an executable file",
            path.display()
        ));
    }

    find_on_path(ADAPTER_BIN)
        .ok_or_else(|| format!("could not find `{ADAPTER_BIN}` on PATH — {ADAPTER_INSTALL_HINT}"))
}

fn sibling_executable(name: &str) -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let dir = exe.parent()?;
    executable_names(name)
        .into_iter()
        .map(|n| dir.join(n))
        .find(|candidate| is_executable(candidate))
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let names = executable_names(name);
    env::split_paths(&env::var_os("PATH")?).find_map(|dir| {
        names
            .iter()
            .map(|n| dir.join(n))
            .find(|candidate| is_executable(candidate))
    })
}

/// Filenames to try for a logical command name, in resolution order.
///
/// On Windows this matters more than it looks: `npm install -g` does not
/// produce a `.exe`, it drops a `codex-acp.cmd` shim next to a
/// `codex-acp` shell script. Probing only `.exe` finds neither, so the
/// documented install instructions would leave the adapter undiscoverable.
/// `PATHEXT` is honoured because that is what the shell itself uses.
fn executable_names(name: &str) -> Vec<String> {
    if !cfg!(windows) {
        return vec![name.to_string()];
    }
    let pathext = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    let mut names: Vec<String> = pathext
        .split(';')
        .filter(|ext| !ext.trim().is_empty())
        .map(|ext| format!("{name}{}", ext.trim().to_ascii_lowercase()))
        .collect();
    // Extension-less last: a bare file is only useful if something else on the
    // system knows how to run it.
    names.push(name.to_string());
    names
}

/// Is this a file we can actually execute?
///
/// `is_file()` alone is not enough: a non-executable file of the right name
/// earlier on `PATH` would win the search and then fail at spawn with 126,
/// even when a real executable sits further along.
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Make a resolved path absolute so `CODEX_PATH` survives a cwd change.
///
/// A relative `PATH` entry or a relative `PFTERMINAL_PATH` would otherwise be
/// re-resolved against whatever directory the adapter happens to be in, which
/// is the opposite of authoritative.
fn absolute(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn print_version() {
    println!("pfterminal-acp {}", env!("CARGO_PKG_VERSION"));
    match resolve_pfterminal() {
        Ok(path) => println!("  pfterminal: {}", path.display()),
        Err(err) => println!("  pfterminal: NOT FOUND ({err})"),
    }
    match resolve_adapter() {
        Ok(path) => println!("  codex-acp:  {}", path.display()),
        Err(err) => println!("  codex-acp:  NOT FOUND ({err})"),
    }
}

fn print_help() {
    println!(
        "\
pfterminal-acp — run PFTerminal as an ACP agent over stdio.

USAGE:
    pfterminal-acp [ADAPTER_ARGS...]

This is a launcher, not an ACP implementation. It resolves the PFTerminal
executable, sets CODEX_PATH, and hands off to the codex-acp adapter.

Arguments are forwarded to the adapter unchanged, except that a leading
--version/-V or --help/-h is handled here and never reaches it.

It is normally started by an ACP client (such as Buzz) rather than by hand;
stdin and stdout carry the protocol.

ENVIRONMENT:
    {PFTERMINAL_PATH_ENV}    pin the PFTerminal executable
    {CODEX_ACP_PATH_ENV}     pin the codex-acp executable
    {CODEX_PATH_ENV}          set by this launcher; any inherited value is overridden

REQUIREMENTS:
    codex-acp must be installed — {ADAPTER_INSTALL_HINT}."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::MutexGuard;

    /// Cargo runs tests in parallel threads, so mutating process environment
    /// is a data race unless serialised. Every test that touches env takes
    /// this first.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn env_guard() -> MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn executable_names_cover_npm_shims_on_windows() {
        let names = executable_names("codex-acp");
        if cfg!(windows) {
            // The documented install is `npm install -g`, which produces a
            // .cmd shim rather than an .exe. Probing only .exe would leave the
            // adapter undiscoverable on a correctly-installed machine.
            assert!(
                names.iter().any(|n| n == "codex-acp.cmd"),
                "npm .cmd shim must be probed: {names:?}"
            );
            assert_eq!(names.last().map(String::as_str), Some("codex-acp"));
        } else {
            assert_eq!(names, vec!["codex-acp".to_string()]);
        }
    }

    #[test]
    fn explicit_path_must_be_executable_not_a_directory() {
        // A directory on PFTERMINAL_PATH is a misconfiguration that must fail
        // loudly; silently falling through to PATH would launch a different
        // agent than the operator pinned.
        let _guard = env_guard();
        let dir = std::env::temp_dir();
        // SAFETY: serialised by ENV_LOCK for the duration of this test.
        unsafe { env::set_var(PFTERMINAL_PATH_ENV, &dir) };
        let result = resolve_pfterminal();
        // SAFETY: serialised by ENV_LOCK for the duration of this test.
        unsafe { env::remove_var(PFTERMINAL_PATH_ENV) };
        assert!(
            result.is_err(),
            "a directory must not resolve as the binary"
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_executable_file_is_rejected() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_guard();
        let dir = std::env::temp_dir().join("pfterminal-acp-nonexec-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("pfterminal");
        std::fs::write(&file, b"#!/bin/sh\n").unwrap();
        // Readable but not executable: the case `is_file()` alone accepted,
        // which would win the PATH search and then fail at spawn.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_executable(&file));

        // SAFETY: serialised by ENV_LOCK for the duration of this test.
        unsafe { env::set_var(PFTERMINAL_PATH_ENV, &file) };
        let result = resolve_pfterminal();
        // SAFETY: serialised by ENV_LOCK for the duration of this test.
        unsafe { env::remove_var(PFTERMINAL_PATH_ENV) };
        let _ = std::fs::remove_dir_all(&dir);

        let err = result.unwrap_err();
        assert!(err.contains("not an executable file"), "unexpected: {err}");
    }

    #[test]
    fn missing_adapter_path_is_reported() {
        let _guard = env_guard();
        // SAFETY: serialised by ENV_LOCK for the duration of this test.
        unsafe { env::set_var(CODEX_ACP_PATH_ENV, "/nonexistent/codex-acp-xyz") };
        let err = resolve_adapter().unwrap_err();
        // SAFETY: serialised by ENV_LOCK for the duration of this test.
        unsafe { env::remove_var(CODEX_ACP_PATH_ENV) };
        assert!(err.contains("not an executable file"), "unexpected: {err}");
    }

    #[test]
    fn absolute_makes_paths_cwd_independent() {
        let cwd = std::env::current_dir().unwrap();
        assert!(absolute(cwd).is_absolute());
        // A path that cannot be canonicalised is returned unchanged rather
        // than discarded.
        let missing = PathBuf::from("/nonexistent/pfterminal-xyz");
        assert_eq!(absolute(missing.clone()), missing);
    }
}
