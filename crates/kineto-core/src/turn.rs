//! A project session's turn: whoever holds it may read and write the
//! project, and only one thread holds it at a time.
//!
//! The session's worker and the threads that run requests waiting on a
//! provider share one turn, so the project is still read and written one step
//! at a time as it was when every request ran on the worker. A request lets
//! go of the turn only while a provider answers ([`outside`]): another request
//! may then take its turn, and the provider calls of several requests run at
//! once.
//!
//! What a request read before letting go may have been changed by another
//! when it takes its turn back. The session never runs two requests that
//! write the same thing at once, and what a request records after its call is
//! read again then, or is its own.

use std::{
    cell::RefCell,
    sync::{Arc, Condvar, Mutex, MutexGuard},
};

/// One project's turn.
#[derive(Debug, Default)]
pub(crate) struct Turn {
    taken: Mutex<bool>,
    freed: Condvar,
}

impl Turn {
    fn take(&self) {
        let mut taken = lock(&self.taken);
        while *taken {
            taken = self
                .freed
                .wait(taken)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        *taken = true;
    }

    fn give_back(&self) {
        *lock(&self.taken) = false;
        self.freed.notify_one();
    }
}

thread_local! {
    /// The turn this thread holds, if any.
    static HELD: RefCell<Option<Arc<Turn>>> = const { RefCell::new(None) };
}

/// Holds `turn` on this thread until dropped.
pub(crate) struct Holding {
    turn: Arc<Turn>,
}

/// Wait for `turn` and hold it on this thread until the result is dropped.
/// A thread holds at most one turn.
pub(crate) fn hold(turn: &Arc<Turn>) -> Holding {
    turn.take();
    HELD.with(|held| {
        let previous = held.borrow_mut().replace(Arc::clone(turn));
        debug_assert!(previous.is_none(), "a thread holds one turn at a time");
    });
    Holding {
        turn: Arc::clone(turn),
    }
}

impl Drop for Holding {
    fn drop(&mut self) {
        HELD.with(|held| held.borrow_mut().take());
        self.turn.give_back();
    }
}

/// Run `call`, a wait on a provider, without the turn this thread holds,
/// and take the turn back before returning. A thread that holds no turn,
/// or has let go of it already, simply runs it.
pub(crate) fn outside<R>(call: impl FnOnce() -> R) -> R {
    let Some(turn) = HELD.with(|held| held.borrow_mut().take()) else {
        return call();
    };
    turn.give_back();
    // Taken back, and held again, even if the call panics, so the holder's
    // drop gives back a turn it holds.
    struct Again(Option<Arc<Turn>>);
    impl Drop for Again {
        fn drop(&mut self) {
            if let Some(turn) = self.0.take() {
                turn.take();
                HELD.with(|held| *held.borrow_mut() = Some(turn));
            }
        }
    }
    let _again = Again(Some(turn));
    call()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicBool, AtomicU32, Ordering},
        thread,
        time::Duration,
    };

    #[test]
    fn one_thread_holds_the_turn_and_a_provider_call_lets_others_take_it() {
        let turn = Arc::new(Turn::default());
        let inside = Arc::new(AtomicU32::new(0));
        let most = Arc::new(AtomicU32::new(0));
        let calling = Arc::new(AtomicU32::new(0));
        let most_calling = Arc::new(AtomicU32::new(0));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let (turn, inside, most, calling, most_calling) = (
                    Arc::clone(&turn),
                    Arc::clone(&inside),
                    Arc::clone(&most),
                    Arc::clone(&calling),
                    Arc::clone(&most_calling),
                );
                thread::spawn(move || {
                    let _holding = hold(&turn);
                    let step = || {
                        let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                        most.fetch_max(now, Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(5));
                        inside.fetch_sub(1, Ordering::SeqCst);
                    };
                    step();
                    outside(|| {
                        let now = calling.fetch_add(1, Ordering::SeqCst) + 1;
                        most_calling.fetch_max(now, Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(150));
                        calling.fetch_sub(1, Ordering::SeqCst);
                    });
                    step();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(
            most.load(Ordering::SeqCst),
            1,
            "the project one step at a time"
        );
        assert!(
            most_calling.load(Ordering::SeqCst) >= 2,
            "provider calls at once"
        );
    }

    #[test]
    fn without_a_turn_a_call_simply_runs_and_a_panicking_call_takes_it_back() {
        assert_eq!(outside(|| 7), 7);
        let turn = Arc::new(Turn::default());
        let holding = hold(&turn);
        // A call within a call lets go once.
        assert_eq!(outside(|| outside(|| 8)), 8);
        let caught = std::panic::catch_unwind(|| outside(|| panic!("the provider failed")));
        assert!(caught.is_err());
        drop(holding);
        // Given back exactly once: another thread takes it.
        let taken = Arc::new(AtomicBool::new(false));
        let other = {
            let (turn, taken) = (Arc::clone(&turn), Arc::clone(&taken));
            thread::spawn(move || {
                let _holding = hold(&turn);
                taken.store(true, Ordering::SeqCst);
            })
        };
        other.join().unwrap();
        assert!(taken.load(Ordering::SeqCst));
    }
}
