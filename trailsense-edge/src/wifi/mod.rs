use embassy_executor::Spawner;
use embassy_net::{DhcpConfig, Stack, StackResources};
use esp_hal::rng::Rng;
use esp_radio::wifi::{WifiController, WifiDevice};

pub mod http;
pub mod tasks;

// Static helper from tutorial
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        STATIC_CELL.uninit().write(($val))
    }};
}
pub struct WifiCtx {
    pub stack: Stack<'static>,
    pub tls_seed: u64,
}

pub fn init(
    spawner: &Spawner,
    rng: &mut Rng,
    wifi_controller: WifiController<'static>,
    wifi_device: WifiDevice<'static>,
) -> WifiCtx {
    let net_seed = rng.random() as u64 | ((rng.random() as u64) << 32);
    let tls_seed = rng.random() as u64 | ((rng.random() as u64) << 32);

    let dhcp_config = DhcpConfig::default();
    let config = embassy_net::Config::dhcpv4(dhcp_config);

    let (stack, runner) = embassy_net::new(
        wifi_device,
        config,
        mk_static!(StackResources<3>, StackResources::<3>::new()),
        net_seed,
    );

    // These must run for the stack to work
    spawner.spawn(tasks::connection(wifi_controller)).unwrap();
    spawner.spawn(tasks::net_task(runner)).unwrap();

    WifiCtx { stack, tls_seed }
}

pub async fn wait_for_connection(stack: Stack<'_>) {
    use embassy_time::{Duration, Timer};
    use esp_println::println;

    println!("Waiting for link to be up");
    while !stack.is_link_up() {
        Timer::after(Duration::from_millis(500)).await;
    }

    println!("Waiting to get IP address...");
    loop {
        if let Some(config) = stack.config_v4() {
            println!("Got IP: {}", config.address);
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }
}
