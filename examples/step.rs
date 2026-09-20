//! Build one step's request and show where the credential goes.
//!
//! Run with a key in the environment:
//!
//! ```sh
//! TYPESAFE_API_KEY=ts-live-... cargo run --example step
//! ```
//!
//! Nothing here embeds a key: it is read at runtime, and the type holding it
//! redacts itself if it ever reaches a log.

use jev_pilot::{
    act::Catalog,
    client::{ENDPOINT, Request},
    credential::ApiKey,
    platform::{Android, Platform},
    step::StepQuestions,
};

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let key = match ApiKey::from_env() {
        Ok(key) => key,
        Err(error) => {
            eprintln!("no credential: {error}");
            eprintln!("the request below is still built; it just cannot be sent.");
            return show(None);
        }
    };
    show(Some(&key))
}

fn show(key: Option<&ApiKey>) -> Result<(), Box<dyn core::error::Error>> {
    let snapshot = Android.parse_hierarchy(include_str!("../tests/fixtures/settings.xml"))?;
    let catalog = Catalog::for_screen(&snapshot, &Android);

    let state = serde_json::json!({
        "platform": Android.name(),
        "screen": snapshot
            .refs()
            .map(|(_, element)| element.describe())
            .collect::<Vec<_>>(),
    });
    let request = Request::new(state, StepQuestions::new("Turn on Wi-Fi", &catalog));

    println!("POST {ENDPOINT}");
    // Note the Debug impl: the secret does not appear.
    println!("Authorization: Bearer {key:?}");
    println!("{}", serde_json::to_string_pretty(&request)?);
    Ok(())
}
