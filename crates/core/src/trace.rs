// SPDX-License-Identifier: MIT OR Apache-2.0
//! The syscall tracer: attach to one process with `ptrace`, record one
//! line per completed syscall, and hand the lines to the TUI.
//!
//! Linux only, and only where the kernel allows the attach: a child of
//! this process always works, another user's process needs privileges.
//! The tracer runs on its own thread (`PTRACE_SEIZE` +
//! `PTRACE_GET_SYSCALL_INFO`, so no per-architecture register layouts),
//! keeps the newest lines in a capped ring, and stops with
//! `PTRACE_INTERRUPT` + `PTRACE_DETACH` when dropped. Every ptrace
//! request — the seize, each wait and resume, and the detach — is made
//! from that one thread: Linux binds a tracee to the thread that
//! attached, and refuses every other thread's requests with `ESRCH`. Every other
//! platform refuses up front: macOS has no user-space ptrace tracing
//! without special entitlements, so ztop reports unavailable there
//! instead of pretending.

/// How many trace lines the ring keeps: the newest line per completed
/// syscall, older ones dropped as the process works.
#[cfg(target_os = "linux")]
const RING_LINES: usize = 512;

/// One running tracer: the shared ring the TUI drains and the thread
/// that fills it. Dropping the handle stops the thread and detaches.
#[cfg(target_os = "linux")]
pub struct Trace {
    pid: u32,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    lines: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(target_os = "linux")]
impl std::fmt::Debug for Trace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Trace").field("pid", &self.pid).finish()
    }
}

#[cfg(target_os = "linux")]
impl Drop for Trace {
    fn drop(&mut self) {
        // Only the tracer thread may make ptrace requests for this
        // tracee, so the drop just raises the flag: the thread sees it
        // within one poll, interrupts, detaches, and exits.
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(target_os = "linux")]
impl Trace {
    /// The pid this tracer follows.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Take every line recorded since the last drain, oldest first.
    pub fn drain(&self) -> Vec<String> {
        let mut ring = self.lines.lock().expect("trace ring");
        ring.drain(..).collect()
    }
}

/// The tracer's public name: the running tracer on Linux (aliased so
/// callers need no platform-specific type), the inert stub elsewhere.
#[cfg(target_os = "linux")]
pub type TraceHandle = Trace;

/// The running tracer's public shape on platforms where tracing is
/// refused: never constructed, but keeps signatures platform-neutral
/// (an empty drain, since no tracer ever runs).
#[cfg(not(target_os = "linux"))]
pub struct TraceHandle;

#[cfg(not(target_os = "linux"))]
impl TraceHandle {
    /// Take every line recorded since the last drain: nothing, as no
    /// tracer ever runs on this platform.
    pub fn drain(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Start tracing `pid`: attach, and record one line per completed
/// syscall until the handle is dropped.
///
/// # Errors
///
/// "syscall tracing is not available on this platform" away from Linux;
/// the OS error text when the attach itself fails (gone pid, no
/// permission, or a kernel without `PTRACE_GET_SYSCALL_INFO`).
#[cfg(not(target_os = "linux"))]
pub fn start(pid: u32) -> Result<TraceHandle, String> {
    let _ = pid;
    Err("syscall tracing is not available on this platform".to_string())
}

#[cfg(target_os = "linux")]
pub fn start(pid: u32) -> Result<TraceHandle, String> {
    let pid_t = pid as libc::pid_t;
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lines = std::sync::Arc::new(std::sync::Mutex::new(
        std::collections::VecDeque::with_capacity(RING_LINES),
    ));
    let thread_stop = stop.clone();
    let thread_lines = lines.clone();
    // The seize happens on the tracer thread, which reports its outcome
    // back before tracing: the thread that attaches is the only one the
    // kernel accepts later requests from.
    let (attached_tx, attached_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
    let thread = std::thread::Builder::new()
        .name(format!("ztop-trace-{pid}"))
        .spawn(move || {
            // Seizing attaches without stopping the tracee and marks
            // this thread as its tracer. PTRACE_O_TRACESYSGOOD is
            // required: without it a syscall stop is reported as a
            // plain SIGTRAP, and GET_SYSCALL_INFO answers "none" for
            // every stop, so no line would ever be recorded.
            let seized = unsafe {
                libc::ptrace(
                    libc::PTRACE_SEIZE,
                    pid_t,
                    0,
                    libc::PTRACE_O_TRACESYSGOOD as *mut libc::c_void,
                )
            };
            if seized != 0 {
                let error = std::io::Error::last_os_error();
                let _ = attached_tx.send(Err(format!("ptrace {pid}: {error}")));
                return;
            }
            let _ = attached_tx.send(Ok(()));
            trace_loop(pid_t, &thread_stop, &thread_lines);
        })
        .map_err(|e| format!("trace thread: {e}"))?;
    let attached = attached_rx
        .recv()
        .unwrap_or_else(|_| Err(format!("ptrace {pid}: the trace thread ended")));
    if let Err(error) = attached {
        let _ = thread.join();
        return Err(error);
    }
    Ok(Trace {
        pid,
        stop,
        lines,
        thread: Some(thread),
    })
}

/// Seize stops report through waitpid even though the tracee is not a
/// child; `__WALL` covers the tracee's threads. Waiting on the tracee's
/// pid specifically — never -1 — so the tracer can never reap another
/// part of the process tree.
#[cfg(target_os = "linux")]
const WALL: libc::c_int = 0x4000_0000;

/// What one wait for the tracee saw.
#[cfg(target_os = "linux")]
enum Waited {
    /// A ptrace stop, with its waitpid status: the tracee is parked
    /// and this thread may read, resume, or detach it.
    Stopped(libc::c_int),
    /// The tracee exited or was killed; there is nothing to detach.
    Gone,
    /// The caller asked to stop while the tracee was running.
    StopRequested,
    /// No stop arrived before the deadline.
    TimedOut,
}

/// Wait for one stop from `pid` without ever blocking: poll `waitpid`
/// with `WNOHANG`, checking `stop` (when given) and `deadline` (when
/// given) each round.
///
/// A blocking `waitpid` here would park the thread forever on a kernel
/// or sandbox that accepts the seize but never delivers the stop, and
/// then `Drop`'s join would hang with it. With the poll, `Drop` always
/// ends the wait within one 20 ms sleep.
///
/// The poll backs off: the next stop of a busy tracee usually comes
/// within microseconds, so the first rounds only yield, then the sleeps
/// grow from 1 ms to 20 ms for a tracee parked in a long syscall. A
/// flat 20 ms sleep per round would cost the tracee about 40 ms per
/// syscall (an entry and an exit stop) — a 25-syscalls-a-second crawl.
#[cfg(target_os = "linux")]
fn wait_stopped(
    pid: libc::pid_t,
    stop: Option<&std::sync::atomic::AtomicBool>,
    deadline: Option<std::time::Instant>,
) -> Waited {
    /// Rounds that only yield before the first sleep.
    const SPIN_ROUNDS: u32 = 64;
    let mut idle_rounds: u32 = 0;
    loop {
        if stop.is_some_and(|stop| stop.load(std::sync::atomic::Ordering::SeqCst)) {
            return Waited::StopRequested;
        }
        let mut status: libc::c_int = 0;
        let waited = unsafe { libc::waitpid(pid, &mut status, WALL | libc::WNOHANG) };
        if waited > 0 {
            if libc::WIFSTOPPED(status) {
                return Waited::Stopped(status);
            }
            if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
                return Waited::Gone;
            }
            continue;
        }
        if waited < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            // ECHILD: the tracee is gone (or reaped elsewhere).
            return Waited::Gone;
        }
        if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
            return Waited::TimedOut;
        }
        // No stop yet: yield or sleep one backed-off poll, then look
        // again.
        idle_rounds = idle_rounds.saturating_add(1);
        if idle_rounds <= SPIN_ROUNDS {
            std::thread::yield_now();
        } else {
            let doublings = (idle_rounds - SPIN_ROUNDS - 1).min(5);
            let millis = (1u64 << doublings).min(20);
            std::thread::sleep(std::time::Duration::from_millis(millis));
        }
    }
}

/// Read the syscall-info record for the stop the tracee is parked in.
/// The buffer size goes in the addr argument and the buffer itself in
/// data — a zero size would leave the buffer unfilled. `None` when the
/// kernel refuses the read.
#[cfg(target_os = "linux")]
fn syscall_info(pid: libc::pid_t) -> Option<libc::ptrace_syscall_info> {
    let mut info: libc::ptrace_syscall_info = unsafe { std::mem::zeroed() };
    let read = unsafe {
        libc::ptrace(
            libc::PTRACE_GET_SYSCALL_INFO,
            pid,
            std::mem::size_of::<libc::ptrace_syscall_info>() as *mut libc::c_void,
            &mut info as *mut libc::ptrace_syscall_info as *mut libc::c_void,
        )
    };
    (read > 0).then_some(info)
}

/// What a ptrace stop is, from its waitpid status and syscall info.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopKind {
    /// A syscall entry or exit: resume with no signal.
    Syscall,
    /// A job-control stop (SIGSTOP, SIGTSTP, SIGTTIN, SIGTTOU) taking
    /// effect: under `PTRACE_SEIZE` it is reported as a
    /// `PTRACE_EVENT_STOP`, and the tracee must stay stopped, so it is
    /// resumed with `PTRACE_LISTEN`, which waits for SIGCONT.
    GroupStop,
    /// Any other event stop, such as the tracer's own interrupt: resume
    /// with no signal.
    Event,
    /// A signal on its way to the tracee: hand it back so it arrives.
    Signal(libc::c_int),
}

#[cfg(target_os = "linux")]
fn stop_kind(status: libc::c_int, info: Option<&libc::ptrace_syscall_info>) -> StopKind {
    if info.is_some_and(|info| info.op != libc::PTRACE_SYSCALL_INFO_NONE) {
        return StopKind::Syscall;
    }
    let event = (status >> 16) & 0xff;
    let signal = libc::WSTOPSIG(status);
    if event == libc::PTRACE_EVENT_STOP {
        return match signal {
            libc::SIGSTOP | libc::SIGTSTP | libc::SIGTTIN | libc::SIGTTOU => StopKind::GroupStop,
            _ => StopKind::Event,
        };
    }
    if event != 0 {
        return StopKind::Event;
    }
    match signal {
        // A syscall stop the info read missed (PTRACE_O_TRACESYSGOOD
        // marks them SIGTRAP | 0x80), or a trap of the tracer's own.
        libc::SIGTRAP => StopKind::Event,
        sig if sig & 0x80 != 0 => StopKind::Syscall,
        // SIGSTOP included: a seized tracee gets no SIGSTOP from the
        // tracer, so one here was sent by someone and must arrive.
        sig => StopKind::Signal(sig),
    }
}

/// The signal to hand back when resuming (or detaching) from a stop:
/// only a signal-delivery stop has one.
#[cfg(target_os = "linux")]
fn passed_signal(kind: StopKind) -> libc::c_int {
    match kind {
        StopKind::Signal(sig) => sig,
        _ => 0,
    }
}

/// Let the tracee go. Called from the tracer thread only, and only
/// while the tracee is parked in a ptrace stop, which is the one state
/// `PTRACE_DETACH` accepts; `signal` is the stop's pending signal, if
/// it was a signal-delivery stop, so detaching never swallows one.
#[cfg(target_os = "linux")]
fn detach(pid: libc::pid_t, signal: libc::c_int) {
    unsafe {
        libc::ptrace(libc::PTRACE_DETACH, pid, 0, signal as *mut libc::c_void);
    }
}

/// How long the tracer waits for the tracee to park after the stop
/// interrupt before giving up on a clean detach.
#[cfg(target_os = "linux")]
const DETACH_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Stop tracing a running tracee: interrupt it, wait for it to park,
/// and detach from that stop. If it never parks (it exited, or the
/// kernel delivered nothing), the detach is still attempted; the
/// kernel also detaches when this tracer thread exits, which it does
/// right after.
#[cfg(target_os = "linux")]
fn interrupt_and_detach(pid: libc::pid_t) {
    unsafe {
        libc::ptrace(libc::PTRACE_INTERRUPT, pid, 0, 0);
    }
    match wait_stopped(pid, None, Some(std::time::Instant::now() + DETACH_WAIT)) {
        Waited::Stopped(status) => {
            let info = syscall_info(pid);
            detach(pid, passed_signal(stop_kind(status, info.as_ref())));
        }
        Waited::Gone => {}
        Waited::StopRequested | Waited::TimedOut => detach(pid, 0),
    }
}

/// The tracer thread, after a successful seize: wait for stops, read
/// one syscall-info record per stop, and resume. Exit events become
/// lines, named by the entry that preceded them; entry events just
/// resume. Ends by detaching, on this same thread, when `stop` is set.
#[cfg(target_os = "linux")]
fn trace_loop(
    pid: libc::pid_t,
    stop: &std::sync::atomic::AtomicBool,
    lines: &std::sync::Mutex<std::collections::VecDeque<String>>,
) {
    // Kick the tracee: a seized tracee that is already running produces
    // no stops on its own, so waiting alone would never see a syscall.
    // The interrupt's stop is where syscall stepping begins. The syscall
    // the tracee was interrupted in is not reported as exiting — the
    // kernel restarts it, so its entry is what steps first — and the
    // loop below skips exit reports whose entry it never saw.
    unsafe {
        libc::ptrace(libc::PTRACE_INTERRUPT, pid, 0, 0);
    }
    // The syscall number of the entry most recently stepped past: an
    // exit record carries no number, so the line's name comes from the
    // entry that preceded the exit.
    let mut current: Option<u64> = None;
    loop {
        let status = match wait_stopped(pid, Some(stop), None) {
            Waited::Stopped(status) => status,
            Waited::Gone | Waited::TimedOut => return,
            Waited::StopRequested => {
                interrupt_and_detach(pid);
                return;
            }
        };
        let info = syscall_info(pid);
        if let Some(info) = &info {
            match info.op {
                libc::PTRACE_SYSCALL_INFO_ENTRY => {
                    current = Some(unsafe { info.u.entry.nr });
                }
                libc::PTRACE_SYSCALL_INFO_EXIT => {
                    // An exit without the entry we traced — the
                    // syscall the tracee was interrupted in at
                    // attach — has no number to name; the restarted
                    // syscall reports its own entry soon after.
                    if let Some(nr) = current.take() {
                        let (sval, is_error) = unsafe { (info.u.exit.sval, info.u.exit.is_error) };
                        let line = if is_error != 0 {
                            format!("{} = -{}", syscall_name(nr), -sval)
                        } else {
                            format!("{} = {sval:#x}", syscall_name(nr))
                        };
                        let mut ring = lines.lock().expect("trace ring");
                        if ring.len() == RING_LINES {
                            ring.pop_front();
                        }
                        ring.push_back(line);
                    }
                }
                _ => {}
            }
        }
        let kind = stop_kind(status, info.as_ref());
        let signal = passed_signal(kind);
        // Parked right now, so a stop request detaches from this very
        // stop instead of resuming first. Detaching from a group-stop
        // leaves the tracee stopped, as job control expects.
        if stop.load(std::sync::atomic::Ordering::SeqCst) {
            detach(pid, signal);
            return;
        }
        // Resume: step to the next syscall stop, or, in a group-stop,
        // listen, so the tracee stays stopped until SIGCONT (which
        // reports another event stop, where stepping resumes). Failures
        // here mean the tracee died under us; the next waitpid says so.
        unsafe {
            if kind == StopKind::GroupStop {
                libc::ptrace(libc::PTRACE_LISTEN, pid, 0, 0);
            } else {
                libc::ptrace(libc::PTRACE_SYSCALL, pid, 0, signal as *mut libc::c_void);
            }
        }
    }
}

/// The syscall number's name, from a curated common-syscall table for
/// the target architecture; anything else reports `syscall N` rather
/// than a guessed name.
#[cfg(target_os = "linux")]
pub fn syscall_name(nr: u64) -> String {
    let table: &[(u64, &str)] = match std::env::consts::ARCH {
        "x86_64" => X86_64_SYSCALLS,
        "aarch64" => AARCH64_SYSCALLS,
        _ => &[],
    };
    table
        .iter()
        .find(|(known, _)| *known == nr)
        .map(|(_, name)| (*name).to_string())
        .unwrap_or_else(|| format!("syscall {nr}"))
}

/// The common x86_64 syscall numbers ztop names; the rest stay numeric.
#[cfg(target_os = "linux")]
const X86_64_SYSCALLS: &[(u64, &str)] = &[
    (0, "read"),
    (1, "write"),
    (2, "open"),
    (3, "close"),
    (4, "stat"),
    (5, "fstat"),
    (7, "poll"),
    (8, "lseek"),
    (9, "mmap"),
    (10, "mprotect"),
    (11, "munmap"),
    (12, "brk"),
    (13, "rt_sigaction"),
    (14, "rt_sigprocmask"),
    (15, "rt_sigreturn"),
    (16, "ioctl"),
    (17, "pread64"),
    (18, "pwrite64"),
    (19, "readv"),
    (20, "writev"),
    (24, "sched_yield"),
    (25, "mremap"),
    (28, "madvise"),
    (32, "dup"),
    (33, "dup2"),
    (35, "nanosleep"),
    (39, "getpid"),
    (41, "socket"),
    (42, "connect"),
    (43, "accept"),
    (44, "sendto"),
    (45, "recvfrom"),
    (46, "sendmsg"),
    (47, "recvmsg"),
    (49, "bind"),
    (50, "listen"),
    (57, "fork"),
    (59, "execve"),
    (60, "exit"),
    (61, "wait4"),
    (62, "kill"),
    (72, "fcntl"),
    (78, "getdents"),
    (79, "getcwd"),
    (87, "unlink"),
    (89, "readlink"),
    (96, "gettimeofday"),
    (98, "getrusage"),
    (101, "ptrace"),
    (102, "getuid"),
    (104, "getgid"),
    (110, "getppid"),
    (131, "tgkill"),
    (186, "gettid"),
    (202, "futex"),
    (218, "set_tid_address"),
    (219, "restart_syscall"),
    (228, "clock_gettime"),
    (230, "clock_nanosleep"),
    (232, "epoll_wait"),
    (233, "epoll_ctl"),
    (257, "openat"),
    (262, "newfstatat"),
    (263, "unlinkat"),
    (267, "readlinkat"),
    (271, "ppoll"),
    (280, "utimensat"),
    (291, "epoll_create1"),
    (292, "dup3"),
    (293, "pipe2"),
    (295, "preadv"),
    (296, "pwritev"),
    (302, "prlimit64"),
    (309, "getcpu"),
    (318, "getrandom"),
    (332, "statx"),
    (425, "io_uring_setup"),
    (426, "io_uring_enter"),
    (435, "clone3"),
    (436, "close_range"),
    (437, "openat2"),
    (439, "faccessat2"),
];

/// The common generic (aarch64) syscall numbers ztop names.
#[cfg(target_os = "linux")]
const AARCH64_SYSCALLS: &[(u64, &str)] = &[
    (19, "eventfd2"),
    (20, "epoll_create1"),
    (21, "epoll_ctl"),
    (22, "epoll_pwait"),
    (23, "dup"),
    (24, "dup3"),
    (25, "fcntl"),
    (26, "inotify_init1"),
    (29, "ioctl"),
    (33, "mknodat"),
    (34, "mkdirat"),
    (35, "unlinkat"),
    (36, "symlinkat"),
    (37, "linkat"),
    (38, "renameat"),
    (43, "statfs"),
    (44, "fstatfs"),
    (45, "truncate"),
    (46, "ftruncate"),
    (48, "faccessat"),
    (49, "chdir"),
    (50, "fchdir"),
    (51, "chroot"),
    (52, "fchmod"),
    (53, "fchmodat"),
    (54, "fchownat"),
    (55, "fchown"),
    (56, "openat"),
    (57, "close"),
    (59, "pipe2"),
    (61, "getdents64"),
    (62, "lseek"),
    (63, "read"),
    (64, "write"),
    (65, "readv"),
    (66, "writev"),
    (67, "pread64"),
    (68, "pwrite64"),
    (73, "ppoll"),
    (78, "readlinkat"),
    (79, "newfstatat"),
    (80, "fstat"),
    (82, "fsync"),
    (83, "fdatasync"),
    (93, "exit"),
    (94, "exit_group"),
    (96, "set_tid_address"),
    (98, "futex"),
    (99, "set_robust_list"),
    (101, "nanosleep"),
    (113, "clock_gettime"),
    (114, "clock_getres"),
    (115, "clock_nanosleep"),
    (116, "syslog"),
    (117, "ptrace"),
    (124, "sched_yield"),
    (129, "kill"),
    (131, "tgkill"),
    (132, "sigaltstack"),
    (134, "rt_sigaction"),
    (135, "rt_sigprocmask"),
    (139, "rt_sigsuspend"),
    (160, "uname"),
    (172, "getpid"),
    (173, "getppid"),
    (174, "getuid"),
    (176, "getgid"),
    (178, "gettid"),
    (179, "sysinfo"),
    (214, "brk"),
    (215, "munmap"),
    (216, "mremap"),
    (220, "clone"),
    (221, "execve"),
    (222, "mmap"),
    (226, "mprotect"),
    (233, "madvise"),
    (260, "wait4"),
    (261, "prlimit64"),
    (278, "getrandom"),
    (279, "memfd_create"),
    (283, "statx"),
    (286, "io_uring_setup"),
    (287, "io_uring_enter"),
    (291, "epoll_pwait2"),
    (435, "clone3"),
    (436, "close_range"),
    (437, "openat2"),
    (439, "faccessat2"),
];

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// A child ztop may trace without privileges (yama's default allows
    /// tracing your own children), kept alive for the test's duration.
    /// Short sleeps in a loop: the shell completes a `wait4` every few
    /// tens of milliseconds, so the test never waits on the one long
    /// sleep a single `sleep 30` would sit inside.
    fn traceable_child() -> std::process::Child {
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("while :; do sleep 0.05; done")
            .spawn()
            .expect("spawn the traced child")
    }

    #[test]
    fn the_tracer_records_syscalls_of_a_child() {
        let child = traceable_child();
        let pid = child.id();
        let finish = |mut child: std::process::Child| {
            child.kill().expect("kill the traced child");
            let _ = child.wait();
        };
        let trace = match start(pid) {
            Ok(trace) => trace,
            Err(error) => {
                // A sandbox may refuse the attach outright (seccomp, a
                // yama scope, a hardened runner): the tracer reports it
                // instead of pretending, and the feature is verified on
                // hosts where the attach works.
                eprintln!("skipping: the environment refuses the attach: {error}");
                finish(child);
                return;
            }
        };
        // The shell itself never sleeps: it forks `sleep` and waits
        // for it, so each round reports a completed `wait4` (the
        // nanosleep happens in the untraced child). Poll with a
        // deadline instead of sleeping a fixed slice — CI machines
        // vary in speed.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut lines: Vec<String> = Vec::new();
        while std::time::Instant::now() < deadline {
            lines.extend(trace.drain());
            if lines.iter().any(|line| line.contains("wait4")) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // No skip for silence: the "seized but no stop ever arrived"
        // symptom decision 0040 put down to CI runners was this
        // tracer's own bug (requests from a thread that had not
        // attached, and no PTRACE_O_TRACESYSGOOD; decision 0060).
        drop(trace);
        finish(child);
        assert!(
            lines.iter().any(|line| line.contains("wait4")),
            "expected a wait4 line, got {lines:?}"
        );
    }

    /// The scheduler state letter of `pid` from `/proc/<pid>/stat`
    /// (`R`, `S`, `t` for stopped by a tracer, `T` for job-stopped).
    fn state_of(pid: u32) -> Option<char> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after_name = stat.rsplit_once(')')?.1;
        after_name.trim_start().chars().next()
    }

    /// Regression: the seize used to run on the caller's thread and
    /// every later ptrace request on the tracer thread. Linux binds a
    /// tracee to the thread that attached, so the other thread's
    /// requests failed with ESRCH: no line was ever recorded, and the
    /// first interrupt left the tracee stopped (`t`) after the trace
    /// closed, frozen until ztop exited. With the whole session on one
    /// thread, lines arrive and dropping the handle lets the process
    /// run again.
    #[test]
    fn closing_a_trace_leaves_the_process_running() {
        let child = traceable_child();
        let pid = child.id();
        let finish = |mut child: std::process::Child| {
            child.kill().expect("kill the traced child");
            let _ = child.wait();
        };
        let trace = match start(pid) {
            Ok(trace) => trace,
            Err(error) => {
                eprintln!("skipping: the environment refuses the attach: {error}");
                finish(child);
                return;
            }
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut lines: Vec<String> = Vec::new();
        while std::time::Instant::now() < deadline && lines.is_empty() {
            lines = trace.drain();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        drop(trace);
        if lines.is_empty() {
            finish(child);
            panic!("the attach worked but no line arrived");
        }
        // Detached: within a second the child must be running or
        // sleeping in its own loop, never parked in a tracing stop.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let mut state = state_of(pid);
        while std::time::Instant::now() < deadline && matches!(state, Some('t' | 'T')) {
            std::thread::sleep(std::time::Duration::from_millis(20));
            state = state_of(pid);
        }
        finish(child);
        assert!(
            !matches!(state, Some('t' | 'T')),
            "the traced process stayed stopped after the trace closed: {state:?}"
        );
    }

    /// A tracee parked inside one long syscall gives the tracer no stop
    /// to detach from; closing must interrupt it, detach from that
    /// interrupt stop, and leave the sleep running.
    #[test]
    fn closing_a_trace_detaches_a_process_inside_a_long_syscall() {
        let child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn the traced child");
        let pid = child.id();
        let finish = |mut child: std::process::Child| {
            child.kill().expect("kill the traced child");
            let _ = child.wait();
        };
        let trace = match start(pid) {
            Ok(trace) => trace,
            Err(error) => {
                eprintln!("skipping: the environment refuses the attach: {error}");
                finish(child);
                return;
            }
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        let closed_at = std::time::Instant::now();
        drop(trace);
        let close_took = closed_at.elapsed();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let mut state = state_of(pid);
        while std::time::Instant::now() < deadline && matches!(state, Some('t' | 'T')) {
            std::thread::sleep(std::time::Duration::from_millis(20));
            state = state_of(pid);
        }
        finish(child);
        assert!(
            !matches!(state, Some('t' | 'T')),
            "the sleeping process stayed stopped after the trace closed: {state:?}"
        );
        assert!(
            close_took < std::time::Duration::from_secs(1),
            "closing took {close_took:?}"
        );
    }

    /// The tracer can start from one thread and be dropped from another
    /// (the TUI owns the handle; nothing pins it to the starting
    /// thread), and a start right after a drop attaches again.
    /// A traced process stopped by job control stays stopped until
    /// SIGCONT, and is traced again after it. The tracer used to
    /// swallow SIGSTOP and resume group-stops, so a traced process
    /// could not be suspended.
    #[test]
    fn a_traced_process_can_be_stopped_and_continued() {
        let child = traceable_child();
        let pid = child.id();
        let finish = |mut child: std::process::Child| {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGCONT) };
            child.kill().expect("kill the traced child");
            let _ = child.wait();
        };
        let trace = match start(pid) {
            Ok(trace) => trace,
            Err(error) => {
                eprintln!("skipping: the environment refuses the attach: {error}");
                finish(child);
                return;
            }
        };
        let quiet_for = |trace: &TraceHandle, window: std::time::Duration| {
            let end = std::time::Instant::now() + window;
            let mut seen = 0;
            while std::time::Instant::now() < end {
                seen += trace.drain().len();
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            seen
        };
        let busy_within = |trace: &TraceHandle| {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while std::time::Instant::now() < end {
                if !trace.drain().is_empty() {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            false
        };
        assert!(busy_within(&trace), "the child makes syscalls while traced");
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGSTOP) };
        // Let the stop land and the last syscalls drain.
        quiet_for(&trace, std::time::Duration::from_millis(300));
        let stopped = quiet_for(&trace, std::time::Duration::from_millis(600));
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGCONT) };
        let resumed = busy_within(&trace);
        drop(trace);
        finish(child);
        assert_eq!(stopped, 0, "a stopped process makes no syscalls");
        assert!(resumed, "SIGCONT resumes the traced process");
    }

    #[test]
    fn a_process_can_be_traced_twice() {
        let child = traceable_child();
        let pid = child.id();
        let finish = |mut child: std::process::Child| {
            child.kill().expect("kill the traced child");
            let _ = child.wait();
        };
        let first = match start(pid) {
            Ok(trace) => trace,
            Err(error) => {
                eprintln!("skipping: the environment refuses the attach: {error}");
                finish(child);
                return;
            }
        };
        std::thread::spawn(move || drop(first))
            .join()
            .expect("drop on another thread");
        let second = start(pid);
        finish(child);
        assert!(
            second.is_ok(),
            "the second attach failed: {:?}",
            second.err()
        );
    }

    #[test]
    fn the_tracer_refuses_a_gone_pid() {
        // A pid nothing owns: the SEIZE fails with an OS error rather
        // than starting a thread that spins.
        assert!(start(399_999_999).is_err());
    }

    #[test]
    fn syscall_names_cover_the_hot_path() {
        // Spot checks on both architectures' tables, plus the numeric
        // fallback for a number no table carries.
        assert_eq!(syscall_name(0), "read");
        assert_eq!(syscall_name(1), "write");
        if std::env::consts::ARCH == "x86_64" {
            assert_eq!(syscall_name(257), "openat");
            assert_eq!(syscall_name(228), "clock_gettime");
        } else if std::env::consts::ARCH == "aarch64" {
            assert_eq!(syscall_name(56), "openat");
            assert_eq!(syscall_name(63), "read");
        }
        assert_eq!(syscall_name(999_999), "syscall 999999");
    }
}
