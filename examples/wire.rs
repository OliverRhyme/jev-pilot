//! Print the request one step puts on the wire, for both platforms.
use jev_pilot::{
    act::Catalog,
    platform::{Android, Ios, Platform},
    step::StepQuestions,
};

fn main() {
    let hierarchy = include_str!("../tests/fixtures/settings.xml");
    let snapshot = Android.parse_hierarchy(hierarchy).expect("fixture parses");

    for platform in [&Android as &dyn Platform, &Ios] {
        let catalog = Catalog::for_screen(&snapshot, platform);
        println!(
            "{}: {} rows, {} operations",
            platform.name(),
            catalog.targets(),
            catalog.operations().len()
        );
    }

    let catalog = Catalog::for_screen(&snapshot, &Android);
    let questions = StepQuestions::new("Turn on Wi-Fi", &catalog);
    println!(
        "{}",
        serde_json::to_string_pretty(&questions).expect("serializes")
    );
}
