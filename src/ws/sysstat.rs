use axum::{
    extract::{
        ws::{Message, WebSocket},
        WebSocketUpgrade,
    },
    response::IntoResponse,
};
use serde::Serialize;
use std::path::Path;

use sysinfo::{Disk, DiskRefreshKind, Disks, Networks, System};
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
    /// Used bytes on the disk holding the home directory.
    disk_used: u64,
    /// Total bytes on that disk; 0 when unknown.
    disk_total: u64,
}

/// Index of the disk whose mount point holds `path` most specifically.
fn disk_for(disks: &[Disk], path: &Path) -> Option<usize> {
    disks
        .iter()
        .enumerate()
        .filter(|(_, d)| path.starts_with(d.mount_point()))
        .max_by_key(|(_, d)| d.mount_point().as_os_str().len())
        .map(|(i, _)| i)
}

pub async fn handle(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    let (Ok(mut sys), Ok(mut networks)) = (System::new(), Networks::new_with_refreshed_list())
    else {
        return;
    };
    let home = dirs::home_dir().unwrap_or_else(|| "/".into());
    let home = home.canonicalize().unwrap_or(home);
    let mut disks =
        Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_storage()).ok();
    let disk = disks.as_ref().and_then(|d| disk_for(d, &home));
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

        let (disk_used, disk_total) = disks
            .as_mut()
            .zip(disk)
            .and_then(|(d, i)| d.get_mut(i))
            .map_or((0, 0), |d| {
                d.refresh_specifics(DiskRefreshKind::nothing().with_storage());
                let total = d.total_space();
                (total.saturating_sub(d.available_space()), total)
            });

        let frame = SysStatFrame {
            cpu,
            mem_used,
            mem_total,
            net_rx,
            net_tx,
            disk_used,
            disk_total,
        };
        let Ok(json) = serde_json::to_string(&frame) else {
            continue;
        };
        if socket.send(Message::Text(json.into())).await.is_err() {
            break;
        }
    }
}
