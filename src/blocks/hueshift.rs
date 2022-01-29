//! Manage display temperature
//!
//! This block displays the current color temperature in Kelvin. When scrolling upon the block the color temperature is changed.
//! A left click on the block sets the color temperature to `click_temp` that is by default to `6500K`.
//! A right click completely resets the color temperature to its default value (`6500K`).
//!
//! # Configuration
//!
//! Key | Values | Required | Default
//! ----|--------|----------|--------
//! `step`        | The step color temperature is in/decreased in Kelvin. | No | `100`
//! `hue_shifter` | Program used to control screen color. | No | Detect automatically. |
//! `max_temp`    | Max color temperature in Kelvin. | No | `10000`
//! `min_temp`    | Min color temperature in Kelvin. | No | `1000`
//! `click_temp`  | Left click color temperature in Kelvin. | No | `6500`
//!
//! # Available Hue Shifters
//!
//! Name | Supports
//! -----|---------
//! `"redshift"`         | X11
//! `"sct"`              | X11
//! `"gammastep"`        | X11 and Wayland
//! `"wl_gammarelay"`    | Wayland
//! `"wl_gammarelay_rs"` | Wayland
//!
//! Note that at the moment, only [`wl_gammarelay`](https://github.com/jeremija/wl-gammarelay) and
//! [`wl_gammarelay_rs`](https://github.com/MaxVerevkin/wl-gammarelay-rs)
//! subscribe to the events and update the bar when the temperature is modified extenrally. Also,
//! these are the only drivers at the moment that work under wayland without flickering.
//!
//! # Example
//!
//! ```toml
//! [[block]]
//! block = "hueshift"
//! hue_shifter = "redshift"
//! step = 50
//! click_temp = 3500
//! ```
//!
//! A hard limit is set for the `max_temp` to `10000K` and the same for the `min_temp` which is `1000K`.
//! The `step` has a hard limit as well, defined to `500K` to avoid too brutal changes.

use super::prelude::*;
use crate::subprocess::{spawn_process, spawn_shell};
use crate::util::has_command;
use futures::future::pending;

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields, default)]
struct HueshiftConfig {
    interval: Seconds,
    max_temp: u16,
    min_temp: u16,
    // TODO: Detect currently defined temperature
    current_temp: u16,
    hue_shifter: Option<HueShifter>,
    step: u16,
    click_temp: u16,
}

impl Default for HueshiftConfig {
    fn default() -> Self {
        Self {
            interval: Seconds::new(5),
            max_temp: 10_000,
            min_temp: 1_000,
            current_temp: 6_500,
            hue_shifter: None,
            step: 100,
            click_temp: 6_500,
        }
    }
}

pub async fn run(config: toml::Value, mut api: CommonApi) -> Result<()> {
    let mut events = api.get_events().await?;
    let config = HueshiftConfig::deserialize(config).config_error()?;

    // limit too big steps at 500K to avoid too brutal changes
    let step = config.step.max(500);
    let max_temp = config.max_temp.min(10_000);
    let min_temp = config.min_temp.clamp(1_000, max_temp);

    let hue_shifter = match config.hue_shifter {
        Some(driver) => driver,
        None => {
            if has_command("wl-gammarelay-rs").await? {
                HueShifter::WlGammarelayRs
            } else if has_command("wl-gammarelay").await? {
                HueShifter::WlGammarelay
            } else if has_command("redshift").await? {
                HueShifter::Redshift
            } else if has_command("sct").await? {
                HueShifter::Sct
            } else if has_command("gammastep").await? {
                HueShifter::Gammastep
            } else if has_command("wlsunset").await? {
                HueShifter::Wlsunset
            } else {
                return Err(Error::new("Cound not detect driver program"));
            }
        }
    };

    let mut driver: Box<dyn HueShiftDriver> = match hue_shifter {
        HueShifter::Redshift => Box::new(Redshift),
        HueShifter::Sct => Box::new(Sct),
        HueShifter::Gammastep => Box::new(Gammastep),
        HueShifter::Wlsunset => Box::new(Wlsunset),
        HueShifter::WlGammarelay => Box::new(WlGammarelayRs::new("wl-gammarelay").await?),
        HueShifter::WlGammarelayRs => Box::new(WlGammarelayRs::new("wl-gammarelay-rs").await?),
    };

    let mut current_temp = config.current_temp;

    loop {
        api.set_text(current_temp.to_string().into());
        api.flush().await?;

        tokio::select! {
            _ = sleep(config.interval.0) => (),
            update = driver.receive_update() => {
                current_temp = update?;
            }
            Some(BlockEvent::Click(click)) = events.recv() => {
                match click.button {
                    MouseButton::Left => {
                        current_temp = config.click_temp;
                        driver.update(current_temp).await?;
                    }
                    MouseButton::Right => {
                        if max_temp > 6500 {
                            current_temp = 6500;
                            driver.reset().await?;
                        } else {
                            current_temp = max_temp;
                            driver.update(current_temp).await?;
                        }
                    }
                    MouseButton::WheelUp => {
                        current_temp = (current_temp + step).min(max_temp);
                        driver.update(current_temp).await?;
                    }
                    MouseButton::WheelDown => {
                        current_temp = current_temp.saturating_sub(step).max(min_temp);
                        driver.update(current_temp).await?;
                    }
                    _ => (),
                }
            }
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
enum HueShifter {
    Redshift,
    Sct,
    Gammastep,
    Wlsunset,
    WlGammarelay,
    WlGammarelayRs,
}

#[async_trait]
trait HueShiftDriver {
    async fn update(&mut self, temp: u16) -> Result<()>;

    async fn reset(&mut self) -> Result<()>;

    async fn receive_update(&mut self) -> Result<u16>;
}

struct Redshift;

#[async_trait]
impl HueShiftDriver for Redshift {
    async fn update(&mut self, temp: u16) -> Result<()> {
        spawn_process("redshift", &["-O", &temp.to_string(), "-P"])
            .error("Failed to set new color temperature using redshift.")
    }
    async fn reset(&mut self) -> Result<()> {
        spawn_process("redshift", &["-x"])
            .error("Failed to set new color temperature using redshift.")
    }
    async fn receive_update(&mut self) -> Result<u16> {
        pending().await
    }
}

struct Sct;

#[async_trait]
impl HueShiftDriver for Sct {
    async fn update(&mut self, temp: u16) -> Result<()> {
        spawn_process("sct", &[&format!("sct {} >/dev/null 2>&1", temp)])
            .error("Failed to set new color temperature using sct.")
    }
    async fn reset(&mut self) -> Result<()> {
        spawn_process("sct", &[]).error("Failed to set new color temperature using sct.")
    }
    async fn receive_update(&mut self) -> Result<u16> {
        pending().await
    }
}

struct Gammastep;

#[async_trait]
impl HueShiftDriver for Gammastep {
    async fn update(&mut self, temp: u16) -> Result<()> {
        spawn_shell(&format!("killall gammastep; gammastep -O {} -P &", temp))
            .error("Failed to set new color temperature using gammastep.")
    }
    async fn reset(&mut self) -> Result<()> {
        spawn_process("gammastep", &["-x"])
            .error("Failed to set new color temperature using gammastep.")
    }
    async fn receive_update(&mut self) -> Result<u16> {
        pending().await
    }
}

struct Wlsunset;

#[async_trait]
impl HueShiftDriver for Wlsunset {
    async fn update(&mut self, temp: u16) -> Result<()> {
        // wlsunset does not have a oneshot option, so set both day and
        // night temperature. wlsunset dose not allow for day and night
        // temperatures to be the same, so increment the day temperature.
        spawn_shell(&format!(
            "killall wlsunset; wlsunset -T {} -t {} &",
            temp + 1,
            temp
        ))
        .error("Failed to set new color temperature using wlsunset.")
    }
    async fn reset(&mut self) -> Result<()> {
        // wlsunset does not have a reset option, so just kill the process.
        // Trying to call wlsunset without any arguments uses the defaults:
        // day temp: 6500K
        // night temp: 4000K
        // latitude/longitude: NaN
        //     ^ results in sun_condition == POLAR_NIGHT at time of testing
        // With these defaults, this results in the the color temperature
        // getting set to 4000K.
        spawn_process("killall", &["wlsunset"])
            .error("Failed to set new color temperature using wlsunset.")
    }
    async fn receive_update(&mut self) -> Result<u16> {
        pending().await
    }
}

struct WlGammarelayRs {
    proxy: WlGammarelayRsBusProxy<'static>,
    updates: zbus::PropertyStream<'static, u16>,
}

impl WlGammarelayRs {
    async fn new(cmd: &str) -> Result<Self> {
        // Make sure the daemon is running
        spawn_process(cmd, &[]).error("Failed to start wl-gammarelay daemon")?;
        sleep(Duration::from_millis(100)).await;

        let conn = crate::util::new_dbus_connection().await?;
        let proxy = WlGammarelayRsBusProxy::new(&conn)
            .await
            .error("Failed to create wl-gammarelay-rs DBus proxy")?;
        let updates = proxy.receive_temperature_changed().await;
        Ok(Self { proxy, updates })
    }
}

#[async_trait]
impl HueShiftDriver for WlGammarelayRs {
    async fn update(&mut self, temp: u16) -> Result<()> {
        self.proxy
            .set_temperature(temp)
            .await
            .error("Failed to set temperature")
    }

    async fn reset(&mut self) -> Result<()> {
        self.update(6500).await
    }

    async fn receive_update(&mut self) -> Result<u16> {
        let update = self.updates.next().await.error("No next update")?;
        update.get().await.error("Failed to get temperature")
    }
}

use zbus::dbus_proxy;

#[dbus_proxy(
    interface = "rs.wl.gammarelay",
    default_service = "rs.wl-gammarelay",
    default_path = "/"
)]
trait WlGammarelayRsBus {
    /// Brightness property
    #[dbus_proxy(property)]
    fn brightness(&self) -> zbus::Result<f64>;
    #[dbus_proxy(property)]
    fn set_brightness(&self, value: f64) -> zbus::Result<()>;

    /// Temperature property
    #[dbus_proxy(property)]
    fn temperature(&self) -> zbus::Result<u16>;
    #[dbus_proxy(property)]
    fn set_temperature(&self, value: u16) -> zbus::Result<()>;
}
