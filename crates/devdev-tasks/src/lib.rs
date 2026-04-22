//! Task Manager & Approval Gate for DevDev.
//!
//! Manages long-lived background work. A task is a unit of ongoing activity
//! that polls on a schedule, reacts to changes, and produces output.

pub mod approval;
pub mod monitor_pr;
pub mod pr_ref;
pub mod registry;
pub mod review;
pub mod scheduler;
pub mod task;

pub use approval::{ApprovalError, ApprovalGate, ApprovalPolicy, ApprovalRequest, ApprovalResponse};
pub use monitor_pr::MonitorPrTask;
pub use pr_ref::PrRef;
pub use registry::TaskRegistry;
pub use review::{parse_review, ParsedReview};
pub use scheduler::TaskScheduler;
pub use task::{Task, TaskError, TaskMessage, TaskStatus};
