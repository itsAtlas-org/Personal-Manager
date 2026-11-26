// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The socket and the folder it lives in, on a real Unix socket (Linux; the file is unix-only). Each
//! test makes its own short-pathed data folder: a socket path has a hard length limit, and a test's
//! folder under the system's temp dir is well inside it.

#![cfg(unix)]

use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::transport::{
    check_len, check_private, from_account, is_shim_cmdline, is_us, probe,
    PlatformTransport as Unix, Probe, StartError, Transport, PID_NAME, SOCKET_NAME,
};

struct World {
    // Held for its drop.
    _dir: tempfile::TempDir,
    data: PathBuf,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().to_path_buf();
        Self { _dir: dir, data }
    }

    fn folder(&self) -> PathBuf {
        self.data.join("mcp")
    }

    fn socket(&self) -> PathBuf {
        self.folder().join(SOCKET_NAME)
    }

    fn pid_file(&self) -> PathBuf {
        self.folder().join(PID_NAME)
    }

    fn bind(&self) -> Result<super::transport::Listener, StartError> {
        Unix::new(&self.data).bind()
    }

    /// The folder, made private by hand (as a first start would).
    fn make_folder(&self) {
        std::fs::create_dir(self.folder()).unwrap();
        std::fs::set_permissions(self.folder(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path).unwrap().mode() & 0o777
}

/// This account's uid: whose a folder this test makes is.
fn euid(w: &World) -> u32 {
    std::fs::metadata(&w.data).unwrap().uid()
}

#[tokio::test]
async fn the_socket_is_0600_in_a_0700_folder_whatever_the_umask() {
    // A umask of 0 is what makes a socket come out 0777 unless PM sets the mode itself.
    // SAFETY: umask only changes the mask of files made while it is set; it is put back at once.
    let old = unsafe { libc::umask(0) };
    let w = World::new();
    let bound = w.bind();
    unsafe { libc::umask(old) };
    let _listener = bound.unwrap();
    assert_eq!(mode(&w.socket()), 0o600);
    assert_eq!(mode(&w.folder()), 0o700);
    assert_eq!(mode(&w.pid_file()), 0o600);
}

#[tokio::test]
async fn the_pid_file_names_this_process() {
    let w = World::new();
    let _listener = w.bind().unwrap();
    let text = std::fs::read_to_string(w.pid_file()).unwrap();
    assert_eq!(text.trim(), std::process::id().to_string());
}

#[tokio::test]
async fn a_connection_from_this_account_is_accepted() {
    let w = World::new();
    let listener = w.bind().unwrap();
    let _client = tokio::net::UnixStream::connect(w.socket()).await.unwrap();
    assert!(listener.accept().await.is_ok());
}

#[tokio::test]
async fn a_folder_open_to_others_is_refused_not_repaired() {
    for bits in [0o755, 0o750, 0o705, 0o770] {
        let w = World::new();
        std::fs::create_dir(w.folder()).unwrap();
        std::fs::set_permissions(w.folder(), std::fs::Permissions::from_mode(bits)).unwrap();
        assert_eq!(w.bind().unwrap_err(), StartError::UnsafeFolder, "{bits:o}");
        // Refused as found: nothing in it, and its mode untouched.
        assert!(!w.socket().exists());
        assert_eq!(mode(&w.folder()), bits);
    }
}

#[tokio::test]
async fn a_link_where_the_folder_goes_is_refused_and_nothing_is_made_through_it() {
    let w = World::new();
    let elsewhere = w.data.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&elsewhere, w.folder()).unwrap();
    assert_eq!(w.bind().unwrap_err(), StartError::UnsafeFolder);
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[tokio::test]
async fn a_folder_that_is_a_file_is_refused() {
    let w = World::new();
    std::fs::write(w.folder(), b"x").unwrap();
    assert_eq!(w.bind().unwrap_err(), StartError::UnsafeFolder);
}

#[test]
fn a_folder_owned_by_someone_else_is_refused() {
    let w = World::new();
    w.make_folder();
    let md = std::fs::symlink_metadata(w.folder()).unwrap();
    assert_eq!(check_private(&md, euid(&w)), Ok(()));
    // The same folder, asked by another account.
    assert_eq!(
        check_private(&md, euid(&w) + 1),
        Err(StartError::UnsafeFolder)
    );
}

#[tokio::test]
async fn a_path_too_long_for_the_address_refuses_and_makes_nothing() {
    let w = World::new();
    let deep = w.data.join("a".repeat(120));
    std::fs::create_dir(&deep).unwrap();
    assert_eq!(
        Unix::new(&deep).bind().unwrap_err(),
        StartError::PathTooLong
    );
    assert!(!deep.join("mcp").exists());
    // The limit is the address's own size.
    let long = Path::new("/").join("x".repeat(110));
    assert_eq!(check_len(&long), Err(StartError::PathTooLong));
    assert_eq!(check_len(Path::new("/tmp/x/mcp/mcp.sock")), Ok(()));
}

#[tokio::test]
async fn a_stale_socket_left_by_a_crash_is_recovered() {
    let w = World::new();
    let first = w.bind().unwrap();
    // A crash: the listener goes with the process, its files stay.
    drop(first);
    assert!(w.socket().exists() && w.pid_file().exists());
    let second = w.bind().unwrap();
    let client = tokio::net::UnixStream::connect(w.socket()).await;
    assert!(client.is_ok(), "the recovered socket answers");
    drop(second);
}

#[tokio::test]
async fn a_live_listener_is_never_taken_over() {
    let w = World::new();
    let live = w.bind().unwrap();
    assert_eq!(w.bind().unwrap_err(), StartError::InUse);
    // The live one is untouched: its socket is still there and still answers.
    assert!(w.socket().exists());
    assert!(tokio::net::UnixStream::connect(w.socket()).await.is_ok());
    drop(live);
}

#[tokio::test]
async fn a_stale_pid_file_is_recovered() {
    let w = World::new();
    drop(w.bind().unwrap());
    // A process that can't exist (past the largest pid).
    std::fs::write(w.pid_file(), "4194999\n").unwrap();
    let _listener = w.bind().unwrap();
    let text = std::fs::read_to_string(w.pid_file()).unwrap();
    assert_eq!(text.trim(), std::process::id().to_string());
    assert_eq!(mode(&w.pid_file()), 0o600);
}

#[tokio::test]
async fn a_pid_file_with_nonsense_in_it_is_stale() {
    let w = World::new();
    drop(w.bind().unwrap());
    std::fs::write(w.pid_file(), "not a pid").unwrap();
    assert!(w.bind().is_ok());
}

#[test]
fn only_a_copy_of_this_program_counts_as_pm() {
    assert!(is_us(std::process::id() as i32));
    // Init is not this program (and may not even be readable). Only Linux can tell: elsewhere `is_us`
    // has no /proc to read and answers yes for any live pid, and the socket probe decides.
    #[cfg(target_os = "linux")]
    assert!(!is_us(1));
}

#[test]
fn the_mcp_shim_is_not_another_copy_of_pm() {
    // `PM --mcp` is the same executable: its argv[1] is what tells it from the app.
    assert!(is_shim_cmdline(b"/opt/pm/pm\0--mcp\0"));
    assert!(is_shim_cmdline(b"pm\0--mcp"));
    assert!(!is_shim_cmdline(b"/opt/pm/pm\0"));
    assert!(!is_shim_cmdline(b"/opt/pm/pm\0--other\0--mcp\0"));
    // Only the argument after the program's name: a program called `--mcp` is no shim.
    assert!(!is_shim_cmdline(b"--mcp\0"));
    assert!(!is_shim_cmdline(b""));
}

/// What the pid-file tests run as a second copy of this program: it only sleeps. It is `ignored` so a
/// plain run never sleeps for it, and a test starts it by name.
#[test]
#[ignore = "started as a child by the pid-file tests"]
fn sleeper_for_the_pid_file_tests() {
    std::thread::sleep(std::time::Duration::from_secs(60));
}

/// A child process, killed and reaped when dropped. Only the Linux pid-file tests start one.
#[cfg(target_os = "linux")]
struct Child(std::process::Child);

#[cfg(target_os = "linux")]
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(target_os = "linux")]
fn run_a_second_copy() -> Child {
    // The same executable as this test run (so `is_us` sees a copy of this program), doing nothing.
    let me = std::env::current_exe().unwrap();
    let child = std::process::Command::new(me)
        .args([
            "--ignored",
            "--exact",
            "mods::mcp::transport_tests::sleeper_for_the_pid_file_tests",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    Child(child)
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_pid_file_naming_a_live_copy_of_pm_stops_the_start() {
    let w = World::new();
    drop(w.bind().unwrap());
    let other = run_a_second_copy();
    let named = format!("{}\n", other.0.id());
    std::fs::write(w.pid_file(), &named).unwrap();
    // The socket is gone (a crash) but the note names a running copy of PM that may be about to bind.
    std::fs::remove_file(w.socket()).unwrap();
    assert_eq!(w.bind().unwrap_err(), StartError::InUse);
    assert_eq!(std::fs::read_to_string(w.pid_file()).unwrap(), named);
    assert!(!w.socket().exists(), "nothing was made");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_pid_file_naming_some_other_program_is_stale() {
    let w = World::new();
    drop(w.bind().unwrap());
    // A running process that is not PM (a reused pid, in real life).
    let other = Child(
        std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap(),
    );
    std::fs::write(w.pid_file(), format!("{}\n", other.0.id())).unwrap();
    let _listener = w.bind().unwrap();
    let text = std::fs::read_to_string(w.pid_file()).unwrap();
    assert_eq!(text.trim(), std::process::id().to_string());
}

#[test]
fn a_connection_counts_only_from_this_account() {
    assert!(from_account(Some(1000), 1000));
    assert!(!from_account(Some(0), 1000));
    assert!(!from_account(Some(1001), 1000));
    // Peer credentials the system couldn't give are no credentials.
    assert!(!from_account(None, 1000));
}

#[tokio::test]
async fn a_listener_that_never_accepts_is_unsure_not_waited_for() {
    let w = World::new();
    w.make_folder();
    // Nothing behind the file: a leftover.
    assert_eq!(probe(&w.folder().join("nothing")), Probe::Stale);
    let listener = std::os::unix::net::UnixListener::bind(w.socket()).unwrap();
    // Fill its queue: it never accepts, so each probe that gets in stays queued.
    let mut queued = 0;
    while probe(&w.socket()) == Probe::Live {
        queued += 1;
        assert!(queued < 100_000, "the queue never filled");
    }
    assert_eq!(probe(&w.socket()), Probe::Unsure);
    // A start over it is told it is in use, at once, and takes nothing of the other's.
    let began = std::time::Instant::now();
    assert_eq!(w.bind().unwrap_err(), StartError::InUse);
    assert!(began.elapsed() < std::time::Duration::from_secs(1));
    assert!(w.socket().exists(), "another run's socket is left alone");
    drop(listener);
}

#[tokio::test]
async fn what_pm_did_not_make_is_never_unlinked() {
    // A regular file where the socket goes.
    let w = World::new();
    w.make_folder();
    std::fs::write(w.socket(), b"mine").unwrap();
    assert_eq!(w.bind().unwrap_err(), StartError::Foreign);
    assert_eq!(std::fs::read(w.socket()).unwrap(), b"mine");

    // A link where the socket goes: refused, and the link and its target are left.
    let w = World::new();
    w.make_folder();
    let target = w.data.join("target");
    std::fs::write(&target, b"theirs").unwrap();
    symlink(&target, w.socket()).unwrap();
    assert_eq!(w.bind().unwrap_err(), StartError::Foreign);
    assert!(std::fs::symlink_metadata(w.socket())
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read(&target).unwrap(), b"theirs");

    // A link where the pid file goes.
    let w = World::new();
    w.make_folder();
    let target = w.data.join("target");
    std::fs::write(&target, b"theirs").unwrap();
    symlink(&target, w.pid_file()).unwrap();
    assert_eq!(w.bind().unwrap_err(), StartError::Foreign);
    assert_eq!(std::fs::read(&target).unwrap(), b"theirs");
}

#[tokio::test]
async fn a_release_removes_what_it_made_and_only_that() {
    let w = World::new();
    let listener = w.bind().unwrap();
    let claim = listener.claim();
    claim.release();
    assert!(!w.socket().exists());
    assert!(!w.pid_file().exists());

    // The path now names something that isn't the socket PM bound: not PM's to remove.
    let w = World::new();
    let listener = w.bind().unwrap();
    let claim = listener.claim();
    std::fs::remove_file(w.socket()).unwrap();
    std::fs::write(w.socket(), b"someone else's").unwrap();
    claim.release();
    assert_eq!(std::fs::read(w.socket()).unwrap(), b"someone else's");
    // Even another socket in its place: a socket, but not the one PM bound.
    std::fs::remove_file(w.socket()).unwrap();
    let _theirs = std::os::unix::net::UnixListener::bind(w.socket()).unwrap();
    claim.release();
    assert!(
        w.socket().exists(),
        "another run's socket is not PM's to unlink"
    );
    drop(listener);
}

#[tokio::test]
async fn a_pid_file_of_another_process_is_not_removed_by_a_release() {
    let w = World::new();
    let listener = w.bind().unwrap();
    let claim = listener.claim();
    std::fs::write(w.pid_file(), "4194999\n").unwrap();
    claim.release();
    assert!(w.pid_file().exists());
}
