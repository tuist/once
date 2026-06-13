pub mod destination;
mod devicectl;
mod simctl;

use std::path::Path;

use anyhow::Result;

pub use destination::{
    parse_destination_kind, AppleDestination, AppleDestinationKind, AppleDestinationSelector,
    AppleDestinationValidation,
};

pub fn list_destinations(include_devices: bool) -> Result<Vec<AppleDestination>> {
    let mut destinations = simctl::list()?;
    if include_devices {
        destinations.extend(devicectl::list()?);
    }
    Ok(destinations)
}

pub fn validate_destination(
    workspace: &Path,
    target_id: &str,
    selector: AppleDestinationSelector,
) -> Result<AppleDestinationValidation> {
    let destinations = list_destinations(selector.kind == AppleDestinationKind::Device)?;
    let destination = destinations
        .into_iter()
        .find(|destination| destination.selector == selector);
    destination::validate(workspace, target_id, selector, destination)
}
