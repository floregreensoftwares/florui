//! [`Executor`]: where [`crate::use_resource`] runs the futures it starts.
//!
//! Component state here is `Rc`-based, not `Arc`-based, so nothing in this
//! crate can require a spawned future to be `Send`. A native host and a
//! browser host each need their own adapter over their own task-running
//! primitive (an OS thread pool, a browser microtask queue); [`LocalExecutor`]
//! is the dependency-light, single-threaded default this crate itself tests
//! against, and that a simple native host can reuse as-is.
//!
//! Polling a future to completion is not the same as providing a timer or
//! an I/O reactor — [`LocalExecutor`] has neither. A fetch that needs to
//! wait on something needs its own source of eventual readiness: bridge a
//! background OS thread via [`crate::blocking::spawn_blocking`] or
//! [`crate::blocking::sleep`], or await a runtime you bring yourself
//! (tokio, async-std) from inside the future [`crate::use_resource`] is
//! given — this crate has no opinion on which.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

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

/// Tracks whether spawned work has become newly pollable since the last
/// [`LocalExecutor::run_until_stalled`], and tells at most one registered
/// listener about it — coalesced, so many wakes before the host reacts
/// call the listener once, but a wake that lands exactly as a run is
/// finishing still flips the flag back on and is reported by the *next*
/// wake, never silently dropped. `wake` can run on any thread (real I/O
/// completes off the UI thread); everything it touches is `Send + Sync`.
struct WakeNotifier {
    ready: AtomicBool,
    listener: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl WakeNotifier {
    fn new() -> Self {
        Self {
            ready: AtomicBool::new(false),
            listener: Mutex::new(None),
        }
    }

    fn notify(&self) {
        if !self.ready.swap(true, Ordering::AcqRel)
            && let Some(listener) = self
                .listener
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
        {
            listener();
        }
    }

    fn take_ready(&self) -> bool {
        self.ready.swap(false, Ordering::AcqRel)
    }
}

/// Wraps a task's real waker (`LocalPool`'s own) so that waking the task
/// also reports readiness through a [`WakeNotifier`] — built on the stable
/// [`Wake`] trait rather than a hand-rolled `RawWaker`, since every part
/// of it is already `Send + Sync`.
struct ForwardingWaker {
    inner: Waker,
    notifier: Arc<WakeNotifier>,
}

impl Wake for ForwardingWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        // Requeue in LocalPool *before* telling anyone to pump it: another
        // thread's run_until_stalled could otherwise wake from `notify`'s
        // channel send and run before this task is actually ready to be
        // found, finding nothing and never draining it. LocalPool's own
        // wake is synchronous, so by the time `notify` runs the task is
        // guaranteed to already be in its ready queue.
        self.inner.wake_by_ref();
        self.notifier.notify();
    }
}

/// Interposes [`ForwardingWaker`] between a spawned task and whichever
/// waker actually polls it, without needing `unsafe` pin projection: both
/// fields are already `Unpin` (a boxed trait object always is), so the
/// whole struct is too.
struct NotifyWaked {
    inner: LocalBoxFuture<'static, ()>,
    notifier: Arc<WakeNotifier>,
}

impl Future for NotifyWaked {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let forwarding = Arc::new(ForwardingWaker {
            inner: cx.waker().clone(),
            notifier: Arc::clone(&self.notifier),
        });
        let waker = Waker::from(forwarding);
        let mut inner_cx = Context::from_waker(&waker);
        self.inner.as_mut().poll(&mut inner_cx)
    }
}

/// The default, dependency-light [`Executor`]: a single-threaded queue of
/// tasks, advanced only when [`Self::run_until_stalled`] is called — never
/// by a background thread. [`Self::on_woken`] is how a host learns *when*
/// to call it again without polling on a timer: real progress (a
/// background thread finishing, an I/O reactor firing) still only
/// happens on its own; this is just the notification that it did.
pub struct LocalExecutor {
    pool: std::cell::RefCell<LocalPool>,
    spawner: futures::executor::LocalSpawner,
    notifier: Arc<WakeNotifier>,
}

impl LocalExecutor {
    pub fn new() -> Self {
        let pool = LocalPool::new();
        let spawner = pool.spawner();
        Self {
            pool: std::cell::RefCell::new(pool),
            spawner,
            notifier: Arc::new(WakeNotifier::new()),
        }
    }

    /// Polls every task that can currently make progress, including ones
    /// woken as a direct result of polling another — until none remain
    /// ready. Does not wait for a task that hasn't been woken.
    pub fn run_until_stalled(&self) {
        self.notifier.take_ready();
        self.pool.borrow_mut().run_until_stalled();
    }

    /// Registers `listener` to run (possibly on a different thread than
    /// whichever calls [`Self::run_until_stalled`]) the moment a spawned
    /// task becomes newly pollable and this hasn't been reported since the
    /// last run. A host wires this to wake its own event loop and then
    /// calls [`Self::run_until_stalled`] back on the thread that owns
    /// these futures — `listener` itself must never touch this executor
    /// directly. Replaces any previously registered listener.
    pub fn on_woken(&self, listener: impl Fn() + Send + Sync + 'static) {
        *self
            .notifier
            .listener
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(listener));
    }
}

impl Default for LocalExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl Executor for LocalExecutor {
    fn spawn(&self, task: LocalBoxFuture<'static, ()>) {
        let wrapped = NotifyWaked {
            inner: task,
            notifier: Arc::clone(&self.notifier),
        };
        self.spawner
            .spawn_local(wrapped)
            .expect("this executor's own LocalPool outlives every task it spawns");
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::testing::manual_future;

    #[test]
    fn run_until_stalled_does_not_advance_a_future_that_was_never_woken() {
        let executor = LocalExecutor::new();
        let (future, _resolver) = manual_future::<i32>();
        let polled = Rc::new(Cell::new(false));
        let polled_in_task = Rc::clone(&polled);

        executor.spawn(Box::pin(async move {
            future.await;
            polled_in_task.set(true);
        }));

        executor.run_until_stalled();
        executor.run_until_stalled();
        assert!(
            !polled.get(),
            "nothing woke the future, so it must not have resolved"
        );
    }

    #[test]
    fn waking_a_task_notifies_the_listener_exactly_once() {
        let executor = LocalExecutor::new();
        let (future, resolver) = manual_future::<i32>();
        let wake_count = Arc::new(AtomicUsize::new(0));
        let wake_count_in_listener = Arc::clone(&wake_count);
        executor.on_woken(move || {
            wake_count_in_listener.fetch_add(1, Ordering::SeqCst);
        });

        executor.spawn(Box::pin(async move {
            future.await;
        }));
        executor.run_until_stalled();
        assert_eq!(
            wake_count.load(Ordering::SeqCst),
            0,
            "the first poll happens synchronously in spawn's caller's own run_until_stalled, \
             not via a wake"
        );

        resolver.resolve(1);
        assert_eq!(wake_count.load(Ordering::SeqCst), 1);

        executor.run_until_stalled();
        assert_eq!(
            wake_count.load(Ordering::SeqCst),
            1,
            "run_until_stalled must clear readiness so an unrelated later wake is still reported"
        );
    }

    #[test]
    fn multiple_wakes_before_run_until_stalled_coalesce_into_one_notification() {
        let executor = LocalExecutor::new();
        let (future_a, resolver_a) = manual_future::<i32>();
        let (future_b, resolver_b) = manual_future::<i32>();
        let wake_count = Arc::new(AtomicUsize::new(0));
        let wake_count_in_listener = Arc::clone(&wake_count);
        executor.on_woken(move || {
            wake_count_in_listener.fetch_add(1, Ordering::SeqCst);
        });

        executor.spawn(Box::pin(async move {
            future_a.await;
        }));
        executor.spawn(Box::pin(async move {
            future_b.await;
        }));
        executor.run_until_stalled();

        resolver_a.resolve(1);
        resolver_b.resolve(2);
        assert_eq!(
            wake_count.load(Ordering::SeqCst),
            1,
            "two wakes before the host reacts must coalesce into a single notification"
        );

        executor.run_until_stalled();
        assert_eq!(wake_count.load(Ordering::SeqCst), 1);
    }
}
