//! Look at a device, and offer it the helper.
//!
//! ```text
//! cargo run --example helper -- <serial>          # report only
//! cargo run --example helper -- <serial> install  # install and enable it
//! ```

use jev_pilot::device::Device as _;
use jev_pilot::device::adb::AdbDevice;
use jev_pilot::device::helper::{BUNDLED, Provision};

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let mut args = std::env::args().skip(1);
    let serial = args.next().ok_or("usage: helper <serial> [install]")?;
    let install = args.next().is_some_and(|word| word == "install");

    let device = AdbDevice::new(serial);
    let provision = device.helper_provision()?;

    println!("device  : {}", device.serial());
    println!("bundled : {} v{}", BUNDLED.package, BUNDLED.version_name);
    println!("state   : {provision:?} — {}", provision.advice());

    if provision.is_ready() {
        let mut device = device.with_helper()?;
        println!(
            "reader  : {}",
            if device.reader().uses_helper() {
                "helper"
            } else {
                "uiautomator CLI"
            }
        );
        // Read repeatedly, so that switching the helper off part way through
        // shows the fallback happening rather than the run ending.
        let rounds: u32 = std::env::var("ROUNDS")
            .ok()
            .and_then(|r| r.parse().ok())
            .unwrap_or(4);
        for round in 1..=rounds {
            let started = std::time::Instant::now();
            let screen = device.observe()?;
            println!(
                "read {round}: {:>3} elements  {:>5}ms  via {}",
                screen.refs().count(),
                started.elapsed().as_millis(),
                if device.reader().uses_helper() {
                    "helper"
                } else {
                    "CLI"
                },
            );
        }
        if let Some(why) = device.reader().why() {
            println!("\nfell back: {why}");
        }
        return Ok(());
    }

    if !install {
        println!(
            "\nThe helper is strongly recommended. Without it every observation costs\n\
             about 2.5s instead of about 50ms, and `uiautomator dump` reports boxes\n\
             whose bottom edge lies above their top for rows scrolled off screen.\n\n\
             It is an accessibility service: once enabled it can read every screen on\n\
             this device, and it answers only on loopback, only to a caller holding a\n\
             token this host generates per run. The source is in `helper/`.\n\n\
             To install it:  cargo run --example helper -- {} install",
            device.serial()
        );
        return Ok(());
    }

    println!("\ninstalling...");
    device.install_helper()?;
    let after = device.helper_provision()?;
    println!("state   : {after:?} — {}", after.advice());
    if after != Provision::Ready {
        return Err("the helper did not come up ready".into());
    }
    Ok(())
}
