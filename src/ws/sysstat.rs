use axum::{
    extract::{
        ws::{Message, WebSocket},
        WebSocketUpgrade,
    },
    response::IntoResponse,
};
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, Thread};
use std::time::Instant;

use sysinfo::{Networks, System};
use tokio::time::{interval, Duration};

#[derive(Serialize)]
pub struct SysStatFrame {
    /// Per-core CPU usage 0–100.
    cpu: Vec<f32>,
    /// Used memory in bytes.
    mem_used: u64,
    /// Total memory in bytes.
    mem_total: u64,
    /// Network received bytes/s (sum across interfaces).
    net_rx: u64,
    /// Network transmitted bytes/s (sum across interfaces).
    net_tx: u64,
    /// The home directory's volume; `None` when unknown or stale.
    disk: Option<DiskSpace>,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct DiskSpace {
    /// Bytes on the volume.
    total: u64,
    /// Bytes the server can still write.
    available: u64,
}

const DISK_INTERVAL: Duration = Duration::from_secs(5);
/// A sample older than this (a hung filesystem) is reported unknown.
const DISK_STALE: Duration = Duration::from_secs(15);

/// One blocking thread samples the home volume for every socket, while any
/// is connected, so a slow filesystem never holds up a frame.
struct DiskSampler {
    latest: Mutex<Option<(Instant, Option<DiskSpace>)>>,
    viewers: AtomicUsize,
    thread: Thread,
}

fn sampler() -> &'static DiskSampler {
    static SAMPLER: OnceLock<DiskSampler> = OnceLock::new();
    SAMPLER.get_or_init(|| {
        let thread = thread::Builder::new()
            .name("disk-sampler".into())
            .spawn(|| loop {
                let s = sampler();
                while s.viewers.load(Ordering::Acquire) == 0 {
                    thread::park();
                }
                let space = dirs::home_dir().and_then(|home| disk_space(&home));
                *s.latest.lock().unwrap() = Some((Instant::now(), space));
                thread::sleep(DISK_INTERVAL);
            })
            .expect("spawn disk sampler")
            .thread()
            .clone();
        DiskSampler {
            latest: Mutex::new(None),
            viewers: AtomicUsize::new(0),
            thread,
        }
    })
}

struct DiskViewer;

impl DiskViewer {
    fn new() -> Self {
        let s = sampler();
        s.viewers.fetch_add(1, Ordering::AcqRel);
        s.thread.unpark();
        DiskViewer
    }

    fn latest(&self) -> Option<DiskSpace> {
        let latest = *sampler().latest.lock().unwrap();
        latest
            .filter(|(at, _)| at.elapsed() < DISK_STALE)
            .and_then(|(_, space)| space)
    }
}

impl Drop for DiskViewer {
    fn drop(&mut self) {
        sampler().viewers.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Space on the filesystem holding `path`, asked of the OS for that path.
#[cfg(unix)]
fn disk_space(path: &Path) -> Option<DiskSpace> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is NUL-terminated and `st` is written on success.
    if unsafe { libc::statvfs(path.as_ptr(), st.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: statvfs succeeded.
    let st = unsafe { st.assume_init() };
    #[allow(clippy::unnecessary_cast)]
    let (frsize, blocks, bavail) = (st.f_frsize as u64, st.f_blocks as u64, st.f_bavail as u64);
    let total = blocks.checked_mul(frsize)?;
    (total > 0).then(|| DiskSpace {
        total,
        available: bavail.saturating_mul(frsize).min(total),
    })
}

#[cfg(windows)]
fn disk_space(path: &Path) -> Option<DiskSpace> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: `wide` is NUL-terminated and the outputs are valid u64s.
    if unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut free) } == 0 {
        return None;
    }
    (total > 0).then(|| DiskSpace {
        total,
        available: available.min(total),
    })
}

pub async fn handle(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    let (Ok(mut sys), Ok(mut networks)) = (System::new(), Networks::new_with_refreshed_list())
    else {
        return;
    };
    let disk = DiskViewer::new();
    let mut tick = interval(Duration::from_secs(1));

    // Prime CPU readings (first call always returns 0 without a prior refresh).
    sys.refresh_cpu_all();
    tick.tick().await;

    loop {
        tick.tick().await;

        sys.refresh_cpu_all();
        sys.refresh_memory();
        networks.refresh(false);

        let cpu: Vec<f32> = sys.cpus().iter().map(|c| c.usage()).collect();
        let mem_used = sys.used_memory();
        let mem_total = sys.total_memory();
        let (net_rx, net_tx) = networks.iter().fold((0u64, 0u64), |(rx, tx), (_, n)| {
            (rx + n.received(), tx + n.transmitted())
        });

        let frame = SysStatFrame {
            cpu,
            mem_used,
            mem_total,
            net_rx,
            net_tx,
            disk: disk.latest(),
        };
        let Ok(json) = serde_json::to_string(&frame) else {
            continue;
        };
        // A peer that went away without closing stops taking frames.
        let sent = tokio::time::timeout(
            Duration::from_secs(10),
            socket.send(Message::Text(json.into())),
        );
        if !matches!(sent.await, Ok(Ok(()))) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_dir_has_space() {
        // Not home: Nix builds set HOME to a missing directory. On Windows
        // this is under the user profile, as home is.
        let space = disk_space(&std::env::temp_dir()).expect("temp dir volume space");
        assert!(space.total > 0);
        assert!(space.available <= space.total);
    }

    #[test]
    fn missing_path_has_no_space() {
        assert!(disk_space(Path::new("/nonexistent/btmux-disk-test")).is_none());
    }
}
