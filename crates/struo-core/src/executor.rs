//! Executor selection and resource-domain admission, independent of action identity.

mod admission;
mod io;
mod selection;
pub(crate) mod staging;

pub use admission::{ExecutionPermit, ExecutorResources};
pub use io::RemoteIoLimits;
pub use selection::{Executor, ResourceDomain};
