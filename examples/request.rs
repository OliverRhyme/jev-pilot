//! Print exactly the JSON body one step sends, and nothing else.
//!
//! ```sh
//! cargo run --quiet --example request | curl -s -X POST \
//!   https://api.typesafe.ai/v1/systemone \
//!   -H "Authorization: Bearer $TYPESAFE_API_KEY" \
//!   -H 'Content-Type: application/json' --data-binary @-
//! ```
use jev_pilot::{
    act::Catalog,
    client::Request,
    platform::{Android, Platform},
    step::StepQuestions,
};

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let goal = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Turn on Wi-Fi".to_owned());
    let snapshot = Android.parse_hierarchy(include_str!("../tests/fixtures/settings.xml"))?;
    let catalog = Catalog::for_screen(&snapshot, &Android);

    let state = serde_json::json!({
        "platform": Android.name(),
        "visible_rows": snapshot.refs().map(|(_, e)| e.describe()).collect::<Vec<_>>(),
    });
    let request = Request::new(state, StepQuestions::new(&goal, &catalog));
    println!("{}", serde_json::to_string(&request)?);
    Ok(())
}
