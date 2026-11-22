//! 背景自動更新的排程(raychiutw/purrdr.host#7):啟動後隨機延遲先檢查一次，之後每 24 小時加抖動。

use std::future::Future;
use std::time::Duration;

use crate::agent_activity::HerdrAgentActivity;
use crate::herdr_socket::HerdrSocket;
use crate::install::InstalledService;
use crate::install::system::ProcessHostSystem;
use crate::push_plugin::PluginUpdater;
use crate::update::{AutomaticOutcome, HttpReleaseSource, UpdateError, Updater};

const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(60 * 60);

/// 啟動後第一次檢查前的等待(1 到 5 分鐘);`random` 在 0..=1。
pub fn first_check_delay(random: f64) -> Duration {
    MINUTE + (4 * MINUTE).mul_f64(random.clamp(0.0, 1.0))
}

/// 下一次檢查前的等待：網路錯誤 1 小時內重試，其他 24 小時加最多 1 小時抖動。
pub fn next_check_delay(outcome: &AutomaticOutcome, random: f64) -> Duration {
    let random = random.clamp(0.0, 1.0);
    match outcome {
        AutomaticOutcome::Failed(UpdateError::Network(_)) => HOUR + (15 * MINUTE).mul_f64(random),
        _ => 24 * HOUR + HOUR.mul_f64(random),
    }
}

/// 依排程一直檢查(`check` 每次做一輪自動檢查);不會結束。
pub async fn run_schedule<C, F>(mut random: impl FnMut() -> f64, mut check: C)
where
    C: FnMut() -> F,
    F: Future<Output = AutomaticOutcome>,
{
    let mut delay = first_check_delay(random());
    loop {
        tokio::time::sleep(delay).await;
        let outcome = check().await;
        delay = next_check_delay(&outcome, random());
    }
}

/// 執行中的是安裝好的主機服務時，在背景開始自動更新排程(`herdr_socket` 用來看 Agent 是否工作中)。
pub fn spawn_for_installed_service(herdr_socket: HerdrSocket, plugin_updater: PluginUpdater) -> Option<tokio::task::JoinHandle<()>> {
    let environment = crate::install::HostEnvironment::from_process().ok()?;
    if !InstalledService::new(environment.clone(), ProcessHostSystem).runs_installed_copy() {
        return None;
    }
    let check = move || {
        let environment = environment.clone();
        let herdr_socket = herdr_socket.clone();
        let plugin_updater = plugin_updater.clone();
        let runtime = tokio::runtime::Handle::current();
        async move {
            let checked = tokio::task::spawn_blocking(move || {
                let location = InstalledService::new(environment, ProcessHostSystem);
                let mut launcher = location.update_launcher();
                let mut agents = HerdrAgentActivity::new(herdr_socket, runtime);
                Updater::new(HttpReleaseSource::from_environment(), location).automatic(&mut launcher, &mut agents)
            })
            .await;
            let outcome = checked.unwrap_or_else(|error| AutomaticOutcome::Failed(UpdateError::Io(error.to_string())));
            println!("auto update: {outcome:?}");
            // 外掛更新不影響主機服務自己的更新:只記 log。
            match plugin_updater.update().await {
                Ok(result) => println!("push plugin update: {result:?}"),
                Err(error) => println!("push plugin update: failed: {error}"),
            }
            outcome
        }
    };
    Some(tokio::spawn(run_schedule(random_fraction, check)))
}

/// 0..=1 的亂數(取不到時用 0.5)。
fn random_fraction() -> f64 {
    let mut bytes = [0u8; 8];
    match getrandom::fill(&mut bytes) {
        Ok(()) => (u64::from_le_bytes(bytes) >> 11) as f64 / (1u64 << 53) as f64,
        Err(_) => 0.5,
    }
}
