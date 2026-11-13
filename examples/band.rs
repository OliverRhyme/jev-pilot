//! Print the step request for a recorded page at band 0 and at band 1.
//!
//! The band is how far beyond the viewport a row is still offered. Whether a
//! wider one helps Jev choose, or only gives it more rows to choose wrongly,
//! is a measurement: send both requests through Jev and compare the
//! confidence on the row that is known to be right, and whether it was offered
//! at all.
//!
//! ```sh
//! cargo run --quiet --features browser --example band -- \
//!   tests/fixtures/browser/search-results.json "Open the second result" \
//!   | while read -r request; do
//!       echo "$request" | curl -s -X POST https://api.typesafe.ai/v1/systemone \
//!         -H "Authorization: Bearer $TYPESAFE_API_KEY" \
//!         -H 'Content-Type: application/json' --data-binary @-
//!     done
//! ```
//!
//! This program calls nothing and needs no key. It writes one request per
//! line to standard output, band 0 first, and says which is which on
//! standard error.

#[cfg(feature = "browser")]
fn main() -> Result<(), Box<dyn core::error::Error>> {
    use jev_pilot::{
        act::Catalog,
        client::Request,
        platform::{
            Platform,
            browser::{BAND_VIEWPORTS, Browser},
        },
        step::StepQuestions,
    };

    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: band <page-read.json> [goal]")?;
    let goal = args
        .next()
        .unwrap_or_else(|| "Open the first result".to_owned());
    let raw = std::fs::read_to_string(&path)?;

    for band in [0.0, BAND_VIEWPORTS] {
        let browser = Browser::with_band(band);
        let snapshot = browser.parse_hierarchy(&raw)?;
        let catalog = Catalog::for_screen(&snapshot, &browser);
        eprintln!("band {band}: {} rows offered", snapshot.rows());
        let state = serde_json::json!({
            "platform": browser.name(),
            "visible_rows": snapshot.refs().map(|(_, e)| e.describe()).collect::<Vec<_>>(),
        });
        let request = Request::new(state, StepQuestions::new(&goal, &catalog));
        println!("{}", serde_json::to_string(&request)?);
    }
    Ok(())
}

#[cfg(not(feature = "browser"))]
fn main() {
    eprintln!(
        "this example needs the `browser` feature: cargo run --features browser --example band"
    );
}
