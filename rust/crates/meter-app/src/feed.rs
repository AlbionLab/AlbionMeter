//! What the UI shows and what it asks for: the contract with the meter, defined in meter-runtime (its `feed` is this
//! crate's former copy, type for type). `meter_runtime::Runtime` publishes it for real; `fake::FakeFeed` and the
//! parity fixture publish the same events without the game.

pub use meter_runtime::feed::*;
