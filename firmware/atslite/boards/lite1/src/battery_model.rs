use atslite_common::battery_model_inc;

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/battery_model.inc.rs"
));
