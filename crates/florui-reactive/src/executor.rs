//! [`Executor`]: where [`crate::use_resource`] runs the futures it starts.
//!
//! Component state here is `Rc`-based, not `Arc`-based, so nothing in this
//! crate can require a spawned future to be `Send`. A native host and a
//! browser host each need their own adapter over their own task-running
//! primitive (an OS thread pool, a browser microtask queue); [`LocalExecutor`]
//! is the dependency-light, single-threaded default this crate itself tests
//! against, and that a simple native host can reuse as-is.

use std::future::Future;
use std::pin::Pin;

use futures::executor::LocalPool;
use futures::task::LocalSpawnExt;

/// A boxed, non-`Send` future — the shape [`Executor::spawn`] accepts and
/// what a fetch closure passed to [`crate::use_resource`] must produce.
pub type LocalBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Runs futures to completion without blocking the caller. Implementations
/// may be backed by anything that can drive a `!Send` future to
/// completion — see the module docs.
pub trait Executor {
    fn spawn(&self, task: LocalBoxFuture<'static, ()>);
}

/// The default, dependency-light [`Executor`]: a single-threaded queue of
/// tasks, advanced only when [`Self::run_until_stalled`] is called — never
/// by a background thread. Real I/O still completes on its own; this just
/// means nothing here polls a task again until something (a host's event
/// loop, or a test) asks it to, which is exactly what makes tests
/// deterministic instead of racing real timing.
pub struct LocalExecutor {
    pool: std::cell::RefCell<LocalPool>,
    spawner: futures::executor::LocalSpawner,
}

impl LocalExecutor {
    pub fn new() -> Self {
        let pool = LocalPool::new();
        let spawner = pool.spawner();
        Self {
            pool: std::cell::RefCell::new(pool),
            spawner,
        }
    }

    /// Polls every task that can currently make progress, including ones
    /// woken as a direct result of polling another — until none remain
    /// ready. Does not wait for a task that hasn't been woken.
    pub fn run_until_stalled(&self) {
        self.pool.borrow_mut().run_until_stalled();
    }
}

impl Default for LocalExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl Executor for LocalExecutor {
    fn spawn(&self, task: LocalBoxFuture<'static, ()>) {
        self.spawner
            .spawn_local(task)
            .expect("this executor's own LocalPool outlives every task it spawns");
    }
}
