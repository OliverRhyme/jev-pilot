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
//! Each request is the one a run's first step on that page would send: the
//! page is served to the pilot as a device, and the judge it asks prints the
//! request it is handed instead of calling anything. So the state and the
//! questions are built by the same code a run uses, and the measurement is of
//! what Jev is really shown.
//!
//! This program calls nothing and needs no key. It writes one request per
//! line to standard output, band 0 first, and says which is which on
//! standard error.

#[cfg(feature = "browser")]
fn main() -> Result<(), Box<dyn core::error::Error>> {
    use jev_pilot::platform::{
        Platform,
        browser::{BAND_VIEWPORTS, Browser},
    };

    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: band <page-read.json> [goal]")?;
    let goal = args
        .next()
        .unwrap_or_else(|| "Open the first result".to_owned());
    let raw = std::fs::read_to_string(&path)?;

    for band in [0.0, BAND_VIEWPORTS] {
        let browser = Browser::with_band(band);
        let rows = browser.parse_hierarchy(&raw)?.rows();
        eprintln!("band {band}: {rows} rows offered");
        println!("{}", first_request(&browser, &raw, &goal)?);
    }
    Ok(())
}

/// The request a run's first step would send on this page, as one line of
/// JSON.
#[cfg(feature = "browser")]
fn first_request(
    browser: &jev_pilot::platform::browser::Browser,
    raw: &str,
    goal: &str,
) -> Result<String, Box<dyn core::error::Error>> {
    use jev_pilot::pilot::{Judge, Pilot};
    use jev_pilot::platform::Platform;
    use jev_pilot::snapshot::Snapshot;
    use jev_pilot::step::{StepAnswers, StepQuestions};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// The recorded page, served as a screen that never changes.
    struct Page<'a> {
        browser: &'a jev_pilot::platform::browser::Browser,
        raw: &'a str,
    }

    impl jev_pilot::device::Device for Page<'_> {
        type Error = jev_pilot::platform::HierarchyError;

        fn observe(&mut self) -> Result<Snapshot, Self::Error> {
            self.browser.parse_hierarchy(self.raw)
        }

        fn perform(&mut self, _: &jev_pilot::device::Command) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    /// Keeps the first request it is asked, and ends the run there.
    struct Keep(Rc<RefCell<Option<String>>>);

    #[derive(Debug)]
    struct Kept;

    impl core::fmt::Display for Kept {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str("the request was kept, and nothing was sent")
        }
    }

    impl core::error::Error for Kept {}

    impl Judge for Keep {
        type Error = Kept;

        fn evaluate(
            &self,
            state: serde_json::Value,
            questions: &StepQuestions<'_>,
        ) -> Result<StepAnswers, Kept> {
            let request = jev_pilot::client::Request::new(&state, questions);
            *self.0.borrow_mut() = serde_json::to_string(&request).ok();
            Err(Kept)
        }
    }

    let kept = Rc::new(RefCell::new(None));
    let mut pilot = Pilot::new(Page { browser, raw }, Keep(Rc::clone(&kept)), browser);
    // The run ends at its first judgement, by design; what matters is the
    // request that judgement was asked.
    let _ = pilot.pursue(goal);
    kept.borrow_mut()
        .take()
        .ok_or_else(|| "the run asked nothing about this page".into())
}

#[cfg(not(feature = "browser"))]
fn main() {
    eprintln!(
        "this example needs the `browser` feature: cargo run --features browser --example band"
    );
}
