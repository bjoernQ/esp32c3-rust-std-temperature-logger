use std::sync::Arc;
use std::thread;
use std::thread::sleep;
use std::time::Duration;

use anyhow::bail;
use embedded_svc::wifi::ClientConfiguration;
use embedded_svc::wifi::ClientConnectionStatus;
use embedded_svc::wifi::ClientIpStatus;
use embedded_svc::wifi::ClientStatus;
use embedded_svc::wifi::Configuration;
use embedded_svc::wifi::Status;
use embedded_svc::wifi::Wifi;
use esp_idf_hal::i2c;
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::prelude::*;
// If using the `binstart` feature of `esp-idf-sys`, always keep this module imported
use esp_idf_sys as _;

use anyhow::Result;
use embedded_svc::mqtt::client::{Connection, Publish, QoS};
use esp_idf_svc::mqtt::client::{EspMqttClient, MqttClientConfiguration};
use esp_idf_svc::netif::*;
use esp_idf_svc::nvs::*;
use esp_idf_svc::sysloop::*;
use esp_idf_svc::wifi::*;

use crate::bmp180::Bmp180;

mod bmp180;

const SSID: &str = env!("SSID");
const PASS: &str = env!("PASSWORD");
const ADAFRUIT_IO_USERNAME: &str = env!("ADAFRUIT_IO_USERNAME");
const ADAFRUIT_IO_KEY: &str = env!("ADAFRUIT_IO_KEY");

fn main() -> Result<()> {
    // Temporary. Will disappear once ESP-IDF 4.4 is released, but for now it is necessary to call this function once,
    // or else some patches to the runtime implemented by esp-idf-sys might not link properly.
    esp_idf_sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take().unwrap();
    let i2c = peripherals.i2c0;
    let sda = peripherals.pins.gpio1;
    let scl = peripherals.pins.gpio2;

    let config = <i2c::config::MasterConfig as Default>::default().baudrate(100.kHz().into());
    let i2c = i2c::Master::<i2c::I2C0, _, _>::new(i2c, i2c::MasterPins { sda, scl }, config)?;

    let mut bmp180 = Bmp180::new(i2c);
    println!("{:?}", bmp180);

    let netif_stack = Arc::new(EspNetifStack::new()?);
    let sys_loop_stack = Arc::new(EspSysLoopStack::new()?);
    let default_nvs = Arc::new(EspDefaultNvs::new()?);

    let _wifi = wifi(
        netif_stack.clone(),
        sys_loop_stack.clone(),
        default_nvs.clone(),
    )?;

    // client_id needs to be unique
    let conf = MqttClientConfiguration {
        client_id: Some("esp32-temperature-logger"),
        keep_alive_interval: Some(Duration::from_secs(120)),
        ..Default::default()
    };

    let (mut client, mut connection) = EspMqttClient::new(
        format!(
            "mqtt://{}:{}@io.adafruit.com:1883",
            ADAFRUIT_IO_USERNAME, ADAFRUIT_IO_KEY
        ),
        &conf,
    )?;

    thread::spawn(move || {
        println!("MQTT Listening for messages");

        while let Some(msg) = connection.next() {
            match msg {
                Err(e) => println!("MQTT Message ERROR: {}", e),
                Ok(msg) => println!("MQTT Message: {:?}", msg),
            }
        }

        println!("MQTT connection loop exit");
    });

    println!("Connected to MQTT");

    loop {
        println!("Before publish");

        bmp180.measure();
        let temperature = bmp180.get_temperature();

        client.publish(
            format!("{}/feeds/temperature", ADAFRUIT_IO_USERNAME),
            QoS::AtMostOnce,
            false,
            format!("{}", temperature).as_bytes(),
        )?;
        println!("Published message");

        sleep(Duration::from_millis(60_000));
    }
}

fn wifi(
    netif_stack: Arc<EspNetifStack>,
    sys_loop_stack: Arc<EspSysLoopStack>,
    default_nvs: Arc<EspDefaultNvs>,
) -> Result<Box<EspWifi>> {
    let mut wifi = Box::new(EspWifi::new(netif_stack, sys_loop_stack, default_nvs)?);

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: SSID.into(),
        password: PASS.into(),
        ..Default::default()
    }))?;

    println!("Wifi configuration set, about to get status");

    wifi.wait_status_with_timeout(Duration::from_secs(20), |status| !status.is_transitional())
        .map_err(|e| anyhow::anyhow!("Unexpected Wifi status: {:?}", e))?;

    let status = wifi.get_status();

    if let Status(
        ClientStatus::Started(ClientConnectionStatus::Connected(ClientIpStatus::Done(
            _ip_settings,
        ))),
        _,
    ) = status
    {
        println!("Wifi connected");
    } else {
        bail!("Unexpected Wifi status: {:?}", status);
    }

    Ok(wifi)
}
