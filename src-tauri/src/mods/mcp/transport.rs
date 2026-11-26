// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The place an app reaches PM (Mods spec 1.16): a Unix socket inside a folder only this account can
//! enter. Never a TCP port, not even on loopback: anything on the machine can connect to one of those,
//! and the point of the socket is that the operating system decides who may.
//!
//! **The folder is the guard.** `data_dir` itself is an ordinary folder (PM makes it with the default
//! mode), so the socket never goes in it. It goes in `data_dir/mcp/`, which PM makes with mode 0700 and
//! then checks at EVERY start: a real directory (not a link someone planted), owned by this account,
//! with no group or other bits. A folder that fails any of that is refused, not repaired, since
//! "repairing" a folder somebody else controls is how a socket ends up where they want it. The socket is
//! then made 0600 on top (belt and braces: the folder is what stops a stranger, the mode is what stops
//! a mistake), and a connection whose peer is not this account is dropped on accept.
//!
//! **Stale files.** A crash leaves the socket file behind and the next start must recover, but never by
//! unlinking something PM didn't make: a path that is not a socket of ours stops the start. A socket
//! that answers a connection is a live listener (another copy of PM) and also stops it; one that
//! refuses is a leftover and is removed. The pid file, `mcp.pid`, is the same kind of note (written
//! 0600, never through a link) and is cleared the same way.
//!
//! Only the Unix arm is built on Linux and macOS. Windows would use a named pipe with an ACL for the
//! current user alone; that arm is not written yet and refuses to start (the section is hidden there,
//! `mcp_offered`), because a pipe with the default ACL is readable by every account.

use std::fmt;
use std::io;

use tokio::io::{AsyncRead, AsyncWrite};

/// The folder (in `data_dir`) holding the socket and the pid file.
pub const DIR_NAME: &str = "mcp";
/// The socket's file name.
pub const SOCKET_NAME: &str = "mcp.sock";
/// The pid file's name. Only the Unix arm keeps one.
#[cfg(unix)]
pub const PID_NAME: &str = "mcp.pid";

/// What a connection is made of: bytes both ways.
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> Stream for T {}

/// Why the server could not start. What it SAYS is PM's fixed sentence: nothing here carries a path or
/// the operating system's own text (that goes to the log where it is first caught).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartError {
    /// The socket's path is longer than the operating system allows.
    #[cfg_attr(not(unix), allow(dead_code))]
    PathTooLong,
    /// `mcp/` is not a real, private folder of this account.
    #[cfg_attr(not(unix), allow(dead_code))]
    UnsafeFolder,
    /// A file is where the socket or pid file goes and PM didn't make it.
    #[cfg_attr(not(unix), allow(dead_code))]
    Foreign,
    /// Something is already answering on the socket.
    #[cfg_attr(not(unix), allow(dead_code))]
    InUse,
    /// This computer's kind of connection point isn't built yet (the Windows arm only).
    #[cfg_attr(unix, allow(dead_code))]
    Unsupported,
    /// A disk or socket step failed.
    Io,
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PathTooLong => {
                "PM can't open its connection point: this computer's data folder path is too long."
            }
            Self::UnsafeFolder => {
                "PM can't open its connection point: its folder isn't private to your account."
            }
            Self::Foreign => "PM can't open its connection point: something else is in its place.",
            Self::InUse => "PM can't open its connection point: another copy of PM is using it.",
            Self::Unsupported => "Connecting apps isn't available on this computer yet.",
            Self::Io => "PM couldn't open its connection point. Try switching it off and on.",
        })
    }
}

impl std::error::Error for StartError {}

impl From<io::Error> for StartError {
    fn from(e: io::Error) -> Self {
        // The kind only: an operating system message can carry a path.
        eprintln!("mcp: the connection point failed ({:?})", e.kind());
        Self::Io
    }
}

/// Something that can open the place apps connect to.
pub trait Transport {
    /// Open it. Must be called with the tokio runtime entered (`Handle::enter`), because the listener
    /// registers with it.
    fn bind(&self) -> Result<Listener, StartError>;
}

#[cfg(unix)]
pub use unix::{Claim, Listener, UnixTransport as PlatformTransport};
// What the tests of the folder's checks reach.
#[cfg(all(unix, test))]
pub(crate) use unix::{
    check_len, check_private, from_account, is_shim_cmdline, is_us, probe, Probe,
};

#[cfg(not(unix))]
pub use other::{Claim, Listener, PipeTransport as PlatformTransport};

#[cfg(unix)]
mod unix {
    use std::fs::{DirBuilder, OpenOptions};
    use std::io::{self, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::fs::{
        DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt,
    };
    use std::path::{Path, PathBuf};

    use super::{StartError, Stream, Transport, DIR_NAME, PID_NAME, SOCKET_NAME};

    /// The bytes `sockaddr_un.sun_path` holds, with its terminating NUL.
    #[cfg(target_os = "linux")]
    const SUN_PATH: usize = 108;
    #[cfg(not(target_os = "linux"))]
    const SUN_PATH: usize = 104;

    fn euid() -> u32 {
        // SAFETY: geteuid has no arguments and cannot fail.
        unsafe { libc::geteuid() }
    }

    /// The socket in `data_dir/mcp/`.
    #[derive(Debug)]
    pub struct UnixTransport {
        data_dir: PathBuf,
    }

    impl UnixTransport {
        pub fn new(data_dir: &Path) -> Self {
            Self {
                data_dir: data_dir.to_owned(),
            }
        }
    }

    /// The folder, made private if it is new and CHECKED either way.
    fn prepare_dir(dir: &Path) -> Result<(), StartError> {
        match std::fs::symlink_metadata(dir) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // One level only: `data_dir` is made by PM's own startup, and a missing parent (a
                // purged install) must not be re-created from here.
                DirBuilder::new().mode(0o700).create(dir)?;
            }
            Err(e) => return Err(e.into()),
        }
        check_private(&std::fs::symlink_metadata(dir)?, euid())
    }

    /// Whether `md` (read without following a link) is a real directory of `euid`'s, closed to
    /// everyone else and open to its owner.
    pub(crate) fn check_private(md: &std::fs::Metadata, euid: u32) -> Result<(), StartError> {
        let ours = md.uid() == euid;
        let private = md.mode() & 0o077 == 0 && md.mode() & 0o700 == 0o700;
        if md.file_type().is_dir() && ours && private {
            Ok(())
        } else {
            Err(StartError::UnsafeFolder)
        }
    }

    pub(crate) fn check_len(socket: &Path) -> Result<(), StartError> {
        use std::os::unix::ffi::OsStrExt;
        // The address takes the path plus a NUL, and PM never `chdir`s to shorten it.
        if socket.as_os_str().as_bytes().len() >= SUN_PATH {
            Err(StartError::PathTooLong)
        } else {
            Ok(())
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    pub(crate) enum Probe {
        Live,
        Stale,
        Unsure,
    }

    /// Whether anything answers on the socket file at `path`. The connect is NON-blocking, so it
    /// returns at once whatever sits behind the file: a listener that never accepts and has a full
    /// queue answers "would block" (`Unsure`) rather than holding the caller, or a thread that the
    /// caller would have to give up on and leak, for as long as it likes. Only a refusal or a missing
    /// file is a leftover.
    pub(crate) fn probe(path: &Path) -> Probe {
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        // SAFETY: an all-zero `sockaddr_un` is a valid (empty) one.
        let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        // The caller checked the length (`check_len`); a longer path is not one PM made.
        if bytes.len() >= addr.sun_path.len() || bytes.contains(&0) {
            return Probe::Unsure;
        }
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (slot, byte) in addr.sun_path.iter_mut().zip(bytes) {
            *slot = *byte as libc::c_char;
        }
        // SAFETY: plain system calls on a descriptor made here and owned by `fd`, which closes it.
        let fd = unsafe {
            let raw = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
            if raw < 0 {
                return Probe::Unsure;
            }
            OwnedFd::from_raw_fd(raw)
        };
        let raw = fd.as_raw_fd();
        // SAFETY: as above; `fcntl` with these commands only reads and sets this descriptor's flags.
        let prepared = unsafe {
            let flags = libc::fcntl(raw, libc::F_GETFL);
            flags >= 0
                && libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) >= 0
                && libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC) >= 0
        };
        if !prepared {
            return Probe::Unsure;
        }
        // SAFETY: `addr` is a filled `sockaddr_un` and the length passed is its size.
        let connected = unsafe {
            libc::connect(
                raw,
                (&raw const addr).cast::<libc::sockaddr>(),
                std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
            )
        };
        if connected == 0 {
            return Probe::Live;
        }
        match io::Error::last_os_error().raw_os_error() {
            Some(libc::ECONNREFUSED | libc::ENOENT) => Probe::Stale,
            _ => Probe::Unsure,
        }
    }

    /// Whether `pid` is a running process (one we may not signal still is).
    fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 only asks whether the process exists.
        let r = unsafe { libc::kill(pid, 0) };
        r == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    /// Whether a process's `/proc/<pid>/cmdline` (NUL-separated) is the `--mcp` shim: the same
    /// executable as PM, started by an app's MCP client, which never binds the socket.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn is_shim_cmdline(cmdline: &[u8]) -> bool {
        cmdline.split(|b| *b == 0).nth(1) == Some(b"--mcp")
    }

    /// Whether `pid` is another copy of PM itself (Linux can tell; elsewhere a running pid is taken
    /// to be one). The same executable is not enough: `PM --mcp` is this program too, and one an app's
    /// client started at login can sit on a pid a crashed PM's note names after a reboot. A copy run
    /// from an AppImage has a per-run mount path for its executable, so a second AppImage copy never
    /// matches here (it is caught by the socket answering, which is the real test).
    pub(crate) fn is_us(pid: i32) -> bool {
        #[cfg(target_os = "linux")]
        {
            let same_program = match (
                std::fs::read_link(format!("/proc/{pid}/exe")),
                std::env::current_exe(),
            ) {
                (Ok(theirs), Ok(ours)) => theirs == ours,
                _ => false,
            };
            same_program
                && std::fs::read(format!("/proc/{pid}/cmdline"))
                    .is_ok_and(|cmdline| !is_shim_cmdline(&cmdline))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = pid;
            true
        }
    }

    /// Clear what a crashed run left, or say why the start must stop. Only ever removes a socket or a
    /// pid file of this account's that is what it looks like.
    fn recover_stale(socket: &Path, pid_file: &Path) -> Result<(), StartError> {
        match std::fs::symlink_metadata(socket) {
            Ok(md) => {
                if !md.file_type().is_socket() || md.uid() != euid() {
                    return Err(StartError::Foreign);
                }
                match probe(socket) {
                    Probe::Live | Probe::Unsure => return Err(StartError::InUse),
                    Probe::Stale => std::fs::remove_file(socket)?,
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        match std::fs::symlink_metadata(pid_file) {
            Ok(md) => {
                if !md.file_type().is_file() || md.uid() != euid() {
                    return Err(StartError::Foreign);
                }
                // The socket is not answering, so the note is stale unless the process it names is
                // another running copy of PM that is about to bind (it cannot be this one: PM binds
                // once at a time, under the supervisor's lock).
                let named = std::fs::read_to_string(pid_file)
                    .ok()
                    .and_then(|t| t.trim().parse::<i32>().ok());
                if let Some(pid) = named {
                    if pid > 0 && pid as u32 != std::process::id() && alive(pid) && is_us(pid) {
                        return Err(StartError::InUse);
                    }
                }
                std::fs::remove_file(pid_file)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    /// The files this start made, so a stop removes those and nothing else.
    #[derive(Debug, Clone)]
    pub struct Claim {
        socket: PathBuf,
        pid_file: PathBuf,
        /// The socket's device and inode as bound: the path might name something else by the time PM
        /// stops, and that is not PM's to unlink.
        ident: (u64, u64),
    }

    impl Claim {
        /// Remove the socket and the pid file if they are still the ones this start made.
        pub fn release(&self) {
            if let Ok(md) = std::fs::symlink_metadata(&self.socket) {
                if md.file_type().is_socket() && (md.dev(), md.ino()) == self.ident {
                    let _ = std::fs::remove_file(&self.socket);
                }
            }
            if let Ok(md) = std::fs::symlink_metadata(&self.pid_file) {
                let ours = std::fs::read_to_string(&self.pid_file)
                    .ok()
                    .and_then(|t| t.trim().parse::<u32>().ok())
                    == Some(std::process::id());
                if md.file_type().is_file() && ours {
                    let _ = std::fs::remove_file(&self.pid_file);
                }
            }
        }
    }

    /// The bound socket.
    /// Whether a connection whose peer reports `peer` as its account (`None`: the system could not
    /// say) is from `own`. A peer that can't be named is not trusted.
    pub(crate) fn from_account(peer: Option<u32>, own: u32) -> bool {
        peer == Some(own)
    }

    #[derive(Debug)]
    pub struct Listener {
        inner: tokio::net::UnixListener,
        claim: Claim,
    }

    impl Listener {
        pub fn claim(&self) -> Claim {
            self.claim.clone()
        }

        /// The next connection from this account. A peer that is anyone else is dropped here and never
        /// seen by the caller.
        pub async fn accept(&self) -> io::Result<Box<dyn Stream>> {
            loop {
                let (stream, _) = self.inner.accept().await?;
                let peer = stream.peer_cred().ok().map(|cred| cred.uid());
                if from_account(peer, euid()) {
                    return Ok(Box::new(stream));
                }
            }
        }
    }

    /// Make the pid file, 0600, through no link.
    fn write_pid_file(pid_file: &Path) -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(pid_file)?;
        writeln!(file, "{}", std::process::id())?;
        file.sync_all()
    }

    impl Transport for UnixTransport {
        fn bind(&self) -> Result<Listener, StartError> {
            let dir = self.data_dir.join(DIR_NAME);
            let socket = dir.join(SOCKET_NAME);
            let pid_file = dir.join(PID_NAME);
            // Before anything is made: a path the address can't hold leaves no folder behind.
            check_len(&socket)?;
            prepare_dir(&dir)?;
            recover_stale(&socket, &pid_file)?;
            let std_listener = std::os::unix::net::UnixListener::bind(&socket)?;
            let md = std::fs::symlink_metadata(&socket)?;
            let claim = Claim {
                socket,
                pid_file,
                ident: (md.dev(), md.ino()),
            };
            // From here on the socket exists and is ours: any failure takes it away again.
            let finish = || -> io::Result<tokio::net::UnixListener> {
                // Explicit, whatever the umask said when the socket was made.
                std::fs::set_permissions(&claim.socket, std::fs::Permissions::from_mode(0o600))?;
                if std::fs::symlink_metadata(&claim.socket)?.mode() & 0o777 != 0o600 {
                    return Err(io::Error::other("the socket's mode did not take"));
                }
                write_pid_file(&claim.pid_file)?;
                std_listener.set_nonblocking(true)?;
                tokio::net::UnixListener::from_std(std_listener)
            };
            match finish() {
                Ok(inner) => Ok(Listener { inner, claim }),
                Err(e) => {
                    claim.release();
                    Err(e.into())
                }
            }
        }
    }
}

#[cfg(not(unix))]
mod other {
    use std::io;
    use std::path::Path;

    use super::{StartError, Stream, Transport};

    /// Not built: see the module doc.
    #[derive(Debug)]
    pub struct PipeTransport;

    impl PipeTransport {
        pub fn new(_data_dir: &Path) -> Self {
            Self
        }
    }

    #[derive(Debug, Clone)]
    pub struct Claim;

    impl Claim {
        pub fn release(&self) {}
    }

    #[derive(Debug)]
    pub struct Listener;

    impl Listener {
        pub fn claim(&self) -> Claim {
            Claim
        }

        pub async fn accept(&self) -> io::Result<Box<dyn Stream>> {
            Err(io::Error::from(io::ErrorKind::Unsupported))
        }
    }

    impl Transport for PipeTransport {
        fn bind(&self) -> Result<Listener, StartError> {
            Err(StartError::Unsupported)
        }
    }
}
